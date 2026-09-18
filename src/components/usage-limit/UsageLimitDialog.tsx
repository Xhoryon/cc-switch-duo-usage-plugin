import { useEffect, useMemo, useState, type ReactNode } from "react";
import { AlertTriangle, Gauge, RotateCcw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import type { Provider } from "@/types";
import type { AppId } from "@/lib/api";
import {
  useResetUsageLimit,
  useSaveUsageLimit,
  useSetUsdCnyRate,
  useUsdCnyRate,
} from "@/lib/query/usageLimit";
import {
  budgetSummary,
  formatMoney,
  formatTokensExact,
} from "./UsageLimitButton";
import type {
  UsageLimitConfig,
  UsageLimitCurrency,
  UsageLimitState,
  UsageLimitStatus,
  UsageLimitType,
} from "@/types/usageLimit";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { extractErrorMessage } from "@/utils/errorUtils";

interface UsageLimitDialogProps {
  provider: Provider;
  appId: AppId;
  isOpen: boolean;
  status: UsageLimitStatus | undefined;
  onClose: () => void;
}

type ResetUnit = "never" | "hours" | "days";

function resetScheduleFromSeconds(seconds?: number | null): {
  unit: ResetUnit;
  value: string;
} {
  if (!seconds || seconds <= 0) {
    return { unit: "never", value: "" };
  }
  if (seconds % (24 * 60 * 60) === 0) {
    return { unit: "days", value: String(seconds / (24 * 60 * 60)) };
  }
  return {
    unit: "hours",
    value: String(Math.max(1, Math.round(seconds / 3600))),
  };
}

/**
 * 使用限额 Dialog（需求 §4-§6、§23-§26）。
 *
 * - 默认 OFF：只展示标题、说明与开关，保持简洁
 * - 开关打开后在同一 Dialog 内展开完整配置（不弹二级 Dialog）
 * - 重置使用量走确认框：usage_start_at 前移，不删历史
 *
 * 视觉遵循 CC Switch 现有氛围：glass 磨砂面板 + bg-muted 分段控件 +
 * 半透明 amber 警示条（与应用内设置行 / ProviderForm 面板同款）。
 */
export function UsageLimitDialog({
  provider,
  appId,
  isOpen,
  status,
  onClose,
}: UsageLimitDialogProps) {
  const { t } = useTranslation();

  const [enabled, setEnabled] = useState(false);
  const [limitType, setLimitType] = useState<UsageLimitType>("money");
  const [currency, setCurrency] = useState<UsageLimitCurrency>("USD");
  const [amountInput, setAmountInput] = useState("");
  const [resetUnit, setResetUnit] = useState<ResetUnit>("never");
  const [resetValue, setResetValue] = useState("");
  const [rateInput, setRateInput] = useState("");
  const [validationError, setValidationError] = useState<string | null>(null);
  const [confirmResetOpen, setConfirmResetOpen] = useState(false);
  const [touchedConfig, setTouchedConfig] = useState(false);

  const { data: savedRate } = useUsdCnyRate();
  const saveMutation = useSaveUsageLimit(provider.id, appId);
  const resetMutation = useResetUsageLimit(provider.id, appId);
  const setRateMutation = useSetUsdCnyRate();

  // 打开或配置到达时，用已保存配置初始化（关闭→重开保留原配置，§24）
  useEffect(() => {
    if (!isOpen) {
      return;
    }
    setEnabled(status?.enabled ?? false);
    setLimitType(status?.limitType ?? "money");
    setCurrency(status?.currency === "CNY" ? "CNY" : "USD");
    setAmountInput(status?.limitAmount ?? "");
    const resetSchedule = resetScheduleFromSeconds(
      status?.resetIntervalSeconds,
    );
    setResetUnit(resetSchedule.unit);
    setResetValue(resetSchedule.value);
    setValidationError(null);
    setTouchedConfig(false);
  }, [
    isOpen,
    status?.enabled,
    status?.limitType,
    status?.currency,
    status?.limitAmount,
    status?.resetIntervalSeconds,
  ]);

  useEffect(() => {
    if (savedRate && !touchedConfig) {
      setRateInput(savedRate);
    }
  }, [savedRate, touchedConfig]);

  const state: UsageLimitState = status?.state ?? "off";
  const percent = useMemo(() => {
    const raw = Number(status?.percentUsed);
    return Number.isFinite(raw) ? raw : 0;
  }, [status?.percentUsed]);

  // 启用行的实时副标题：与卡片图标 tooltip 同一套文案
  const enableRowSubtitle = useMemo(() => {
    if (!status || !status.enabled) {
      return t("usageLimit.tooltipOff");
    }
    if (state === "exhausted") {
      return t("usageLimit.tooltipExhausted");
    }
    const summary = budgetSummary(status);
    return summary
      ? `${summary.used} / ${summary.limit}`
      : t("usageLimit.title");
  }, [status, state, t]);

  // 当前输入币种下的已用金额（CNY 优先用后端换算值，缺失时按本地汇率估算）
  const usedMoneyInInputCurrency = useMemo(() => {
    if (!status) {
      return 0;
    }
    if (currency === "CNY") {
      // 注意 Number(null) === 0：必须显式判空才能走汇率换算兜底
      const converted =
        status.usedMoneyInCurrency != null
          ? Number(status.usedMoneyInCurrency)
          : NaN;
      if (Number.isFinite(converted)) {
        return converted;
      }
      const rate = Number(rateInput) || 0;
      return Number(status.usedMoneyUsd) * rate;
    }
    return Number(status.usedMoneyUsd);
  }, [status, currency, rateInput]);

  const limitValue = useMemo(() => Number(amountInput), [amountInput]);
  const hasValidAmount = useMemo(() => {
    if (limitType === "token") {
      return /^\d+$/.test(amountInput.trim()) && Number(amountInput.trim()) > 0;
    }
    return Number.isFinite(limitValue) && limitValue > 0;
  }, [limitType, amountInput, limitValue]);

  const usedValue =
    limitType === "token"
      ? (status?.usedTokens ?? 0)
      : usedMoneyInInputCurrency;
  const remainingValue = hasValidAmount ? limitValue - usedValue : 0;

  const showEnforcementWarning =
    enabled && status && status.enforcement !== "active";
  const showUnpricedWarning =
    enabled && limitType === "money" && (status?.unpricedRequestCount ?? 0) > 0;

  const handleSave = () => {
    const trimmed = amountInput.trim().replace(/,/g, "");
    // 前端预校验（后端仍会权威校验）：非法输入不保存
    if (limitType === "money") {
      const value = Number(trimmed);
      if (!trimmed || !Number.isFinite(value) || value <= 0) {
        setValidationError(t("usageLimit.errorInvalidAmount"));
        return;
      }
    } else if (!/^\d+$/.test(trimmed) || Number(trimmed) <= 0) {
      setValidationError(t("usageLimit.errorInvalidTokens"));
      return;
    }
    if (limitType === "money" && currency === "CNY") {
      const rate = Number(rateInput.trim());
      if (!rateInput.trim() || !Number.isFinite(rate) || rate <= 0) {
        setValidationError(t("usageLimit.errorInvalidRate"));
        return;
      }
    }

    let resetIntervalSeconds: number | null = null;
    if (resetUnit !== "never") {
      const value = Number(resetValue.trim());
      if (!Number.isSafeInteger(value) || value <= 0) {
        setValidationError(t("usageLimit.errorInvalidReset"));
        return;
      }
      const multiplier = resetUnit === "days" ? 24 * 60 * 60 : 60 * 60;
      resetIntervalSeconds = value * multiplier;
      if (!Number.isSafeInteger(resetIntervalSeconds)) {
        setValidationError(t("usageLimit.errorInvalidReset"));
        return;
      }
    }

    setValidationError(null);
    const config: UsageLimitConfig = {
      enabled,
      limitType,
      currency: limitType === "money" ? currency : null,
      limitAmount: trimmed,
      resetIntervalSeconds,
    };
    saveMutation.mutate(config, {
      onSuccess: () => {
        // CNY 模式顺带持久化本地汇率
        if (limitType === "money" && currency === "CNY") {
          const rate = rateInput.trim();
          if (rate && Number(rate) > 0) {
            setRateMutation.mutate(rate, {
              onError: (error) => {
                toast.error(extractErrorMessage(error));
              },
            });
          }
        }
        toast.success(t("usageLimit.saved"));
        onClose();
      },
      onError: (error) => {
        toast.error(extractErrorMessage(error));
      },
    });
  };

  const handleReset = () => {
    resetMutation.mutate(undefined, {
      onSuccess: () => {
        toast.success(t("usageLimit.resetDone"));
        setConfirmResetOpen(false);
      },
      onError: (error) => {
        toast.error(extractErrorMessage(error));
        setConfirmResetOpen(false);
      },
    });
  };

  const toggleType = (type: UsageLimitType) => {
    setLimitType(type);
    setTouchedConfig(true);
    setValidationError(null);
  };

  const hasSavedConfig = Boolean(status?.limitAmount);
  const canReset = Boolean(status?.enabled || hasSavedConfig);

  return (
    <>
      <Dialog open={isOpen} onOpenChange={(open) => !open && onClose()}>
        <DialogContent className="sm:max-w-md" data-testid="usage-limit-dialog">
          <DialogHeader>
            <DialogTitle>
              {t("usageLimit.title")}
              <span className="ml-2 text-sm font-normal text-muted-foreground">
                {provider.name}
              </span>
            </DialogTitle>
            <DialogDescription>{t("usageLimit.description")}</DialogDescription>
          </DialogHeader>

          {/* 启用开关：设置页同款图标卡片行 */}
          <div className="glass flex items-center gap-3 rounded-xl border border-white/10 p-3.5">
            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-primary/10">
              <Gauge className="h-4 w-4 text-primary" />
            </div>
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium leading-tight">
                {t("usageLimit.enable")}
              </p>
              <p className="mt-0.5 truncate text-xs text-muted-foreground">
                {enableRowSubtitle}
              </p>
            </div>
            <Switch
              checked={enabled}
              onCheckedChange={(checked) => {
                setEnabled(checked);
                setTouchedConfig(true);
                setValidationError(null);
              }}
              aria-label={t("usageLimit.enable")}
              data-testid="usage-limit-toggle"
            />
          </div>

          {enabled && (
            <div className="glass space-y-4 rounded-xl border border-white/10 p-4">
              {/* 限制方式 */}
              <div className="space-y-2">
                <span className="text-sm font-medium">
                  {t("usageLimit.limitType")}
                </span>
                <div className="grid grid-cols-2 gap-1 rounded-lg bg-muted p-1">
                  <SegmentOption
                    active={limitType === "money"}
                    label={t("usageLimit.money")}
                    onClick={() => toggleType("money")}
                  />
                  <SegmentOption
                    active={limitType === "token"}
                    label={t("usageLimit.tokens")}
                    onClick={() => toggleType("token")}
                  />
                </div>
              </div>

              {limitType === "money" && (
                <>
                  {/* 币种 */}
                  <div className="space-y-2">
                    <span className="text-sm font-medium">
                      {t("usageLimit.currency")}
                    </span>
                    <div className="grid grid-cols-2 gap-1 rounded-lg bg-muted p-1">
                      <SegmentOption
                        active={currency === "CNY"}
                        label={t("usageLimit.cny")}
                        onClick={() => {
                          setCurrency("CNY");
                          setTouchedConfig(true);
                          setValidationError(null);
                        }}
                      />
                      <SegmentOption
                        active={currency === "USD"}
                        label={t("usageLimit.usd")}
                        onClick={() => {
                          setCurrency("USD");
                          setTouchedConfig(true);
                          setValidationError(null);
                        }}
                      />
                    </div>
                  </div>

                  {/* CNY 本地汇率（不依赖在线汇率 API） */}
                  {currency === "CNY" && (
                    <div className="space-y-1.5">
                      <label
                        htmlFor="usage-limit-rate"
                        className="text-sm font-medium"
                      >
                        {t("usageLimit.exchangeRate")}
                      </label>
                      <Input
                        id="usage-limit-rate"
                        value={rateInput}
                        onChange={(e) => {
                          setRateInput(e.target.value);
                          setTouchedConfig(true);
                        }}
                        placeholder="7.20"
                        inputMode="decimal"
                        className="h-9"
                      />
                      <p className="text-xs text-muted-foreground">
                        {t("usageLimit.exchangeRateHint")}
                      </p>
                    </div>
                  )}

                  {/* 最大金额 */}
                  <div className="space-y-1.5">
                    <label
                      htmlFor="usage-limit-amount"
                      className="text-sm font-medium"
                    >
                      {t("usageLimit.maxAmount")}
                    </label>
                    <Input
                      id="usage-limit-amount"
                      data-testid="usage-limit-amount"
                      value={amountInput}
                      onChange={(e) => {
                        setAmountInput(e.target.value);
                        setTouchedConfig(true);
                        setValidationError(null);
                      }}
                      placeholder="100.00"
                      inputMode="decimal"
                      className="h-9"
                    />
                  </div>
                </>
              )}

              {limitType === "token" && (
                <div className="space-y-1.5">
                  <label
                    htmlFor="usage-limit-amount"
                    className="text-sm font-medium"
                  >
                    {t("usageLimit.maxTokens")}
                  </label>
                  <Input
                    id="usage-limit-amount"
                    data-testid="usage-limit-amount"
                    value={amountInput}
                    onChange={(e) => {
                      setAmountInput(e.target.value);
                      setTouchedConfig(true);
                      setValidationError(null);
                    }}
                    placeholder="5,000,000"
                    inputMode="numeric"
                    className="h-9"
                  />
                </div>
              )}

              {/* 自动重置周期：窗口到期后在下一次状态读取或请求时惰性切换 */}
              <div className="space-y-1.5">
                <label
                  htmlFor="usage-limit-reset-unit"
                  className="text-sm font-medium"
                >
                  {t("usageLimit.resetSchedule")}
                </label>
                <div className="flex gap-2">
                  <Select
                    value={resetUnit}
                    onValueChange={(value) => {
                      const nextUnit = value as ResetUnit;
                      setResetUnit(nextUnit);
                      if (nextUnit !== "never" && !resetValue) {
                        setResetValue("1");
                      }
                      setTouchedConfig(true);
                      setValidationError(null);
                    }}
                  >
                    <SelectTrigger
                      id="usage-limit-reset-unit"
                      data-testid="usage-limit-reset-unit"
                      className="flex-1"
                    >
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="never">
                        {t("usageLimit.resetNever")}
                      </SelectItem>
                      <SelectItem value="hours">
                        {t("usageLimit.resetHours")}
                      </SelectItem>
                      <SelectItem value="days">
                        {t("usageLimit.resetDays")}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  {resetUnit !== "never" && (
                    <Input
                      id="usage-limit-reset-value"
                      data-testid="usage-limit-reset-value"
                      value={resetValue}
                      onChange={(event) => {
                        setResetValue(event.target.value);
                        setTouchedConfig(true);
                        setValidationError(null);
                      }}
                      min="1"
                      step="1"
                      type="number"
                      inputMode="numeric"
                      className="h-9 w-24"
                    />
                  )}
                </div>
                <p className="text-xs text-muted-foreground">
                  {t("usageLimit.resetHint")}
                </p>
              </div>

              {/* enforcement 边界提示（§20 / §21） */}
              {showEnforcementWarning && (
                <WarningBanner>
                  {status.enforcement === "unsupported_credential"
                    ? t("usageLimit.enforcementUnsupported")
                    : t("usageLimit.enforcementProxyDisabled")}
                </WarningBanner>
              )}

              {/* 未知定价提示（§10） */}
              {showUnpricedWarning && (
                <WarningBanner>
                  {t("usageLimit.pricingUnavailable", {
                    count: status?.unpricedRequestCount ?? 0,
                    models: (status?.unpricedModels ?? []).join(", "),
                  })}
                </WarningBanner>
              )}

              {/* 用量与进度（§26：百分比不 clamp，进度条视觉封顶 100%） */}
              <div className="space-y-2.5 rounded-lg border border-border/60 p-3">
                <UsageRow
                  label={t("usageLimit.used")}
                  value={
                    limitType === "token"
                      ? `${formatTokensExact(status?.usedTokens ?? 0)} tokens`
                      : formatMoney(
                          usedMoneyInInputCurrency.toString(),
                          currency,
                        )
                  }
                />
                {hasValidAmount && (
                  <UsageRow
                    label={t("usageLimit.remaining")}
                    value={
                      limitType === "token"
                        ? `${formatTokensExact(remainingValue)} tokens`
                        : formatMoney(remainingValue.toString(), currency)
                    }
                  />
                )}
                <div className="pt-0.5">
                  <ProgressBar percent={percent} state={state} />
                  {hasValidAmount && (
                    <p className="mt-1.5 text-right text-xs tabular-nums text-muted-foreground">
                      {percent.toFixed(1)}%
                    </p>
                  )}
                </div>
              </div>
            </div>
          )}

          {validationError && (
            <p
              className="text-sm text-red-600 dark:text-red-400"
              role="alert"
              data-testid="usage-limit-error"
            >
              {validationError}
            </p>
          )}

          <DialogFooter className="gap-2 sm:gap-0">
            {canReset && (
              <Button
                variant="ghost"
                className="mr-auto text-muted-foreground hover:text-red-600 dark:hover:text-red-400"
                onClick={() => setConfirmResetOpen(true)}
                data-testid="usage-limit-reset"
              >
                <RotateCcw className="mr-1 h-4 w-4" />
                {t("usageLimit.reset")}
              </Button>
            )}
            <Button variant="outline" onClick={onClose}>
              {t("common.cancel")}
            </Button>
            <Button
              onClick={handleSave}
              disabled={saveMutation.isPending}
              data-testid="usage-limit-save"
            >
              {saveMutation.isPending ? t("common.saving") : t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        isOpen={confirmResetOpen}
        title={t("usageLimit.resetConfirmTitle")}
        message={t("usageLimit.resetConfirmMessage")}
        confirmText={t("usageLimit.reset")}
        cancelText={t("common.cancel")}
        variant="info"
        zIndex="nested"
        pending={resetMutation.isPending}
        onConfirm={() => handleReset()}
        onCancel={() => setConfirmResetOpen(false)}
      />
    </>
  );
}

/** bg-muted 轨道分段选择（与设置页 pill 风格一致） */
function SegmentOption({
  active,
  label,
  onClick,
}: {
  active: boolean;
  label: string;
  onClick: () => void;
}) {
  return (
    <Button
      type="button"
      size="sm"
      variant="ghost"
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "h-8 font-medium",
        active && "bg-background shadow-sm hover:bg-background",
        !active && "text-muted-foreground hover:text-foreground",
      )}
    >
      {label}
    </Button>
  );
}

function UsageRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between text-sm">
      <span className="text-muted-foreground">{label}</span>
      <span className="font-medium tabular-nums">{value}</span>
    </div>
  );
}

/** 进度条：视觉最多填满 100%，颜色随状态变化 */
function ProgressBar({
  percent,
  state,
}: {
  percent: number;
  state: UsageLimitState;
}) {
  const clamped = Math.max(0, Math.min(100, percent));
  return (
    <div
      className="h-2 w-full overflow-hidden rounded-full bg-muted"
      data-testid="usage-limit-progress"
    >
      <div
        className={cn(
          "h-full rounded-full transition-all",
          state === "exhausted" && "bg-red-500",
          state === "warning" && "bg-amber-500",
          (state === "active" || state === "off") && "bg-emerald-500",
        )}
        style={{ width: `${clamped}%` }}
      />
    </div>
  );
}

/** 半透明警示条（与应用 amber 提示同款色调，深浅色自适应） */
function WarningBanner({ children }: { children: ReactNode }) {
  return (
    <div
      className="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 p-2.5 text-xs leading-relaxed text-amber-600 dark:text-amber-400"
      data-testid="usage-limit-warning"
    >
      <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-amber-500 dark:text-amber-400" />
      <span>{children}</span>
    </div>
  );
}
