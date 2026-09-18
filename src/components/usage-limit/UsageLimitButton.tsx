import { memo } from "react";
import { Gauge } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { useUsageLimitStatus } from "@/lib/query/usageLimit";
import { isProxyAppId } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import type {
  UsageLimitCurrency,
  UsageLimitState,
  UsageLimitStatus,
} from "@/types/usageLimit";

/** Token 友好显示：532 / 12.4K / 1.25M / 18.2M */
export function formatTokensCompact(tokens: number): string {
  if (tokens < 1000) {
    return String(tokens);
  }
  if (tokens < 1_000_000) {
    return `${(tokens / 1000).toFixed(1).replace(/\.0$/, "")}K`;
  }
  if (tokens < 10_000_000) {
    return `${(tokens / 1_000_000).toFixed(2).replace(/\.?0+$/, "")}M`;
  }
  return `${(tokens / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
}

/** Token 精确显示：1,245,821 */
export function formatTokensExact(tokens: number): string {
  return tokens.toLocaleString("en-US");
}

/** 金额显示：$12.34 / ¥88.50 */
export function formatMoney(amount: string, currency: UsageLimitCurrency) {
  const symbol = currency === "CNY" ? "¥" : "$";
  const value = Number(amount);
  if (!Number.isFinite(value)) {
    return `${symbol}${amount}`;
  }
  return `${symbol}${value.toFixed(2)}`;
}

interface BudgetSummary {
  used: string;
  limit: string;
}

/** 图标 Tooltip 里的简略用量（$2.41 / $10 或 1.2M / 5M） */
export function budgetSummary(
  status: UsageLimitStatus,
): BudgetSummary | undefined {
  if (!status.enabled || !status.limitType || !status.limitAmount) {
    return undefined;
  }
  if (status.limitType === "money") {
    const currency: UsageLimitCurrency =
      status.currency === "CNY" ? "CNY" : "USD";
    const used =
      currency === "CNY"
        ? (status.usedMoneyInCurrency ?? status.usedMoneyUsd)
        : status.usedMoneyUsd;
    return {
      used: formatMoney(used, currency),
      limit: formatMoney(status.limitAmount, currency),
    };
  }
  return {
    used: formatTokensCompact(Number(status.usedTokens)),
    limit: formatTokensCompact(Number(status.limitAmount)),
  };
}

interface UsageLimitButtonProps {
  providerId: string;
  appId: AppId;
  onOpen: () => void;
}

/**
 * Provider Card 上的使用限额图标。
 *
 * 四种状态（需求 §3.2）：
 * - OFF：未开启限额，普通灰色
 * - ACTIVE：已开启、用量正常
 * - WARNING：>= 80% 且 < 100%
 * - EXHAUSTED：>= 100%，明显警示
 *
 * 仅对可走 Local Proxy 的应用显示（claude / codex / gemini / grokbuild）；
 * 非代理应用没有强制执行能力，不渲染。外层组件先做应用类型闸门，
 * 内层组件才挂查询 hook（保证 hook 顺序稳定）。
 */
function UsageLimitButtonImpl({
  providerId,
  appId,
  onOpen,
}: UsageLimitButtonProps) {
  if (!isProxyAppId(appId)) {
    return null;
  }
  return (
    <UsageLimitButtonInner
      providerId={providerId}
      appId={appId}
      onOpen={onOpen}
    />
  );
}

function UsageLimitButtonInner({
  providerId,
  appId,
  onOpen,
}: UsageLimitButtonProps) {
  const { t } = useTranslation();
  const { data: status } = useUsageLimitStatus(providerId, appId);

  const state: UsageLimitState = status?.state ?? "off";
  const summary = status ? budgetSummary(status) : undefined;

  let title = t("usageLimit.title");
  if (status) {
    switch (state) {
      case "off":
        title = t("usageLimit.tooltipOff", { defaultValue: "使用限额：关闭" });
        break;
      case "exhausted":
        title = t("usageLimit.tooltipExhausted", {
          defaultValue: "已达到使用限额",
        });
        break;
      case "warning":
      case "active":
        title = summary
          ? `${summary.used} / ${summary.limit}`
          : t("usageLimit.title");
        break;
    }
  }

  return (
    <TooltipProvider delayDuration={250}>
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            size="icon"
            variant="ghost"
            onClick={onOpen}
            aria-label={t("usageLimit.title")}
            className={cn(
              "h-8 w-8 p-1",
              state === "off" && "text-muted-foreground hover:text-foreground",
              state === "active" && "text-emerald-600 dark:text-emerald-400",
              state === "warning" && "text-amber-600 dark:text-amber-400",
              state === "exhausted" &&
                "text-red-600 dark:text-red-400 hover:text-red-700 dark:hover:text-red-300",
            )}
          >
            <Gauge className="h-4 w-4" />
          </Button>
        </TooltipTrigger>
        <TooltipContent className="flex items-center gap-1.5">
          <span
            aria-hidden="true"
            className={cn(
              "h-1.5 w-1.5 rounded-full",
              state === "off" && "bg-muted-foreground",
              state === "active" && "bg-emerald-400",
              state === "warning" && "bg-amber-400",
              state === "exhausted" && "bg-red-400",
            )}
          />
          {title}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

export const UsageLimitButton = memo(UsageLimitButtonImpl);
