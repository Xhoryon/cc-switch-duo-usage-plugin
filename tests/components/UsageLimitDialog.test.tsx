import { render, screen, fireEvent } from "@testing-library/react";
import { toast } from "sonner";
import { describe, it, expect, vi, beforeEach } from "vitest";

import { UsageLimitDialog } from "@/components/usage-limit/UsageLimitDialog";
import {
  UsageLimitButton,
  formatTokensCompact,
  formatMoney,
} from "@/components/usage-limit/UsageLimitButton";
import type { UsageLimitStatus } from "@/types/usageLimit";
import type { Provider } from "@/types";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, options?: Record<string, unknown>) => {
      if (options && "count" in options) {
        return `${key}:${String(options.count)}`;
      }
      return key;
    },
  }),
}));

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() },
}));

const saveMutate = vi.fn();
const resetMutate = vi.fn();
const setRateMutate = vi.fn();

let mockStatus: UsageLimitStatus | undefined;
let mockRate = "7.2";
let savePending = false;
let saveError: Error | null = null;

vi.mock("@/lib/query/usageLimit", () => ({
  usageLimitKeys: {
    all: ["usage-limit"],
    status: (p: string, a: string) => ["usage-limit", "status", p, a],
  },
  useUsageLimitStatus: () => ({ data: mockStatus }),
  useUsdCnyRate: () => ({ data: mockRate }),
  useSaveUsageLimit: () => ({
    mutate: (
      config: unknown,
      handlers?: {
        onSuccess?: () => void;
        onError?: (e: Error) => void;
      },
    ) => {
      saveMutate(config);
      if (saveError) {
        handlers?.onError?.(saveError);
      } else {
        handlers?.onSuccess?.();
      }
    },
    isPending: savePending,
  }),
  useResetUsageLimit: () => ({
    mutate: (
      _vars: unknown,
      handlers?: { onSuccess?: () => void; onError?: (e: Error) => void },
    ) => {
      resetMutate();
      handlers?.onSuccess?.();
    },
    isPending: false,
  }),
  useSetUsdCnyRate: () => ({
    mutate: (rate: string) => setRateMutate(rate),
    isPending: false,
  }),
}));

const provider: Provider = {
  id: "p1",
  name: "Test Provider",
  settingsConfig: {},
};

function baseStatus(
  overrides: Partial<UsageLimitStatus> = {},
): UsageLimitStatus {
  return {
    providerId: "p1",
    appType: "claude",
    enabled: false,
    limitType: null,
    currency: null,
    limitAmount: null,
    usageStartAt: null,
    usedMoneyUsd: "0",
    usedMoneyInCurrency: null,
    usedTokens: 0,
    percentUsed: null,
    state: "off",
    unpricedRequestCount: 0,
    unpricedModels: [],
    enforcement: "active",
    maskedCredential: "sk-****ABCD",
    ...overrides,
  };
}

function renderDialog(status?: UsageLimitStatus) {
  return render(
    <UsageLimitDialog
      provider={provider}
      appId="claude"
      isOpen={true}
      status={status}
      onClose={() => {}}
    />,
  );
}

describe("UsageLimitDialog", () => {
  beforeEach(() => {
    saveMutate.mockReset();
    resetMutate.mockReset();
    setRateMutate.mockReset();
    saveError = null;
    savePending = false;
    mockStatus = undefined;
    mockRate = "7.2";
  });

  it("renders OFF by default with no config section", () => {
    renderDialog(baseStatus());
    const toggle = screen.getByTestId("usage-limit-toggle");
    expect(toggle).not.toBeChecked();
    expect(screen.queryByText("usageLimit.limitType")).toBeNull();
    expect(screen.getByText("usageLimit.description")).toBeTruthy();
  });

  it("expands the config area when toggled on (no second dialog)", () => {
    renderDialog(baseStatus());
    fireEvent.click(screen.getByTestId("usage-limit-toggle"));
    expect(screen.getByText("usageLimit.limitType")).toBeTruthy();
    expect(screen.getByText("usageLimit.maxAmount")).toBeTruthy();
  });

  it("starts from saved config when limit is enabled", () => {
    renderDialog(
      baseStatus({
        enabled: true,
        limitType: "money",
        currency: "USD",
        limitAmount: "10",
        state: "active",
        percentUsed: "24.00",
        usedMoneyUsd: "2.41",
      }),
    );
    const toggle = screen.getByTestId("usage-limit-toggle");
    expect(toggle).toBeChecked();
    expect(screen.getByTestId("usage-limit-amount")).toHaveValue("10");
    expect(screen.getByText("$2.41")).toBeTruthy();
    expect(screen.getByText("$7.59")).toBeTruthy();
  });

  it("money mode: switching to CNY shows the exchange rate input and converted usage", () => {
    renderDialog(
      baseStatus({
        enabled: true,
        limitType: "money",
        currency: "USD",
        limitAmount: "10",
        usedMoneyUsd: "2.41",
      }),
    );
    expect(screen.queryByLabelText("usageLimit.exchangeRate")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "usageLimit.cny" }));
    expect(screen.getByLabelText("usageLimit.exchangeRate")).toBeTruthy();
    // 2.41 * 7.2 = 17.35
    expect(screen.getByText("¥17.35")).toBeTruthy();
  });

  it("money mode: back to USD hides rate input", () => {
    renderDialog(
      baseStatus({
        enabled: true,
        limitType: "money",
        currency: "CNY",
        limitAmount: "72",
      }),
    );
    expect(screen.getByLabelText("usageLimit.exchangeRate")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "usageLimit.usd" }));
    expect(screen.queryByLabelText("usageLimit.exchangeRate")).toBeNull();
  });

  it("token mode shows the token input and token usage", () => {
    renderDialog(
      baseStatus({
        enabled: true,
        limitType: "token",
        limitAmount: "5000000",
        usedTokens: 1245821,
        percentUsed: "24.92",
      }),
    );
    expect(screen.getByText("usageLimit.maxTokens")).toBeTruthy();
    expect(screen.getByText("1,245,821 tokens")).toBeTruthy();
    expect(screen.getByText("3,754,179 tokens")).toBeTruthy();
  });

  it("blocks invalid input from saving", () => {
    renderDialog(baseStatus());
    fireEvent.click(screen.getByTestId("usage-limit-toggle"));
    // 空输入
    fireEvent.click(screen.getByTestId("usage-limit-save"));
    expect(screen.getByTestId("usage-limit-error")).toBeTruthy();
    expect(saveMutate).not.toHaveBeenCalled();

    // 负数
    const input = screen.getByTestId("usage-limit-amount");
    fireEvent.change(input, { target: { value: "-5" } });
    fireEvent.click(screen.getByTestId("usage-limit-save"));
    expect(saveMutate).not.toHaveBeenCalled();

    // token 模式小数
    fireEvent.click(screen.getByRole("button", { name: "usageLimit.tokens" }));
    fireEvent.change(input, { target: { value: "3.5" } });
    fireEvent.click(screen.getByTestId("usage-limit-save"));
    expect(saveMutate).not.toHaveBeenCalled();
  });

  it("saves a valid money limit", () => {
    renderDialog(baseStatus());
    fireEvent.click(screen.getByTestId("usage-limit-toggle"));
    fireEvent.change(screen.getByTestId("usage-limit-amount"), {
      target: { value: "100.00" },
    });
    fireEvent.click(screen.getByTestId("usage-limit-save"));
    expect(saveMutate).toHaveBeenCalledWith({
      enabled: true,
      limitType: "money",
      currency: "USD",
      limitAmount: "100.00",
      resetIntervalSeconds: null,
    });
  });

  it("saves an hourly reset schedule", () => {
    renderDialog(baseStatus());
    fireEvent.click(screen.getByTestId("usage-limit-toggle"));
    fireEvent.click(screen.getByTestId("usage-limit-reset-unit"));
    fireEvent.click(
      screen.getByRole("option", { name: "usageLimit.resetHours" }),
    );
    fireEvent.change(screen.getByTestId("usage-limit-reset-value"), {
      target: { value: "6" },
    });
    fireEvent.change(screen.getByTestId("usage-limit-amount"), {
      target: { value: "100" },
    });
    fireEvent.click(screen.getByTestId("usage-limit-save"));

    expect(saveMutate).toHaveBeenCalledWith({
      enabled: true,
      limitType: "money",
      currency: "USD",
      limitAmount: "100",
      resetIntervalSeconds: 21_600,
    });
  });

  it("restores a daily schedule and can switch back to never", () => {
    renderDialog(
      baseStatus({
        enabled: true,
        limitType: "token",
        limitAmount: "1000",
        resetIntervalSeconds: 2 * 24 * 60 * 60,
      }),
    );
    expect(screen.getByTestId("usage-limit-reset-unit")).toHaveTextContent(
      "usageLimit.resetDays",
    );
    expect(screen.getByTestId("usage-limit-reset-value")).toHaveValue(2);

    fireEvent.click(screen.getByTestId("usage-limit-reset-unit"));
    fireEvent.click(
      screen.getByRole("option", { name: "usageLimit.resetNever" }),
    );
    fireEvent.click(screen.getByTestId("usage-limit-save"));

    expect(saveMutate).toHaveBeenCalledWith({
      enabled: true,
      limitType: "token",
      currency: null,
      limitAmount: "1000",
      resetIntervalSeconds: null,
    });
  });

  it("shows an understandable error when the API rejects the save", () => {
    saveError = new Error("Provider 不存在: p1");
    renderDialog(baseStatus());
    fireEvent.click(screen.getByTestId("usage-limit-toggle"));
    fireEvent.change(screen.getByTestId("usage-limit-amount"), {
      target: { value: "100" },
    });
    fireEvent.click(screen.getByTestId("usage-limit-save"));
    expect(saveMutate).toHaveBeenCalled();
    // API 错误走 sonner toast 提示；内联 usage-limit-error 只用于本地校验失败
    expect(toast.error).toHaveBeenCalledWith("Provider 不存在: p1");
  });

  it("shows progress with percent not clamped when exhausted", () => {
    renderDialog(
      baseStatus({
        enabled: true,
        limitType: "money",
        currency: "USD",
        limitAmount: "10",
        usedMoneyUsd: "10.42",
        percentUsed: "104.20",
        state: "exhausted",
      }),
    );
    expect(screen.getByText("104.2%")).toBeTruthy();
    const bar = screen.getByTestId("usage-limit-progress")
      .firstElementChild as HTMLElement;
    // 进度条视觉封顶 100%，数值展示不 clamp
    expect(bar.style.width).toBe("100%");
  });

  it("hides reset until a config exists, then confirms before resetting", () => {
    // 无任何配置：不显示重置
    renderDialog(baseStatus());
    expect(screen.queryByTestId("usage-limit-reset")).toBeNull();

    // 已有配置：显示重置，点击后需确认
    const { unmount } = render(
      <UsageLimitDialog
        provider={provider}
        appId="claude"
        isOpen={true}
        status={baseStatus({
          enabled: true,
          limitType: "token",
          limitAmount: "1000000",
          usedTokens: 600,
        })}
        onClose={() => {}}
      />,
    );
    fireEvent.click(screen.getByTestId("usage-limit-reset"));
    // ConfirmDialog 弹出（标题出现）
    expect(screen.getByText("usageLimit.resetConfirmTitle")).toBeTruthy();
    // 确认框的确认按钮（Dialog footer 也有同名重置按钮，取确认框里的那个）
    const resetButtons = screen.getAllByRole("button", {
      name: "usageLimit.reset",
    });
    fireEvent.click(resetButtons[resetButtons.length - 1]);
    expect(resetMutate).toHaveBeenCalled();
    unmount();
  });

  it("shows enforcement warning when proxy is disabled", () => {
    renderDialog(
      baseStatus({
        enabled: true,
        limitType: "money",
        currency: "USD",
        limitAmount: "10",
        enforcement: "proxy_disabled",
      }),
    );
    expect(
      screen.getByText("usageLimit.enforcementProxyDisabled"),
    ).toBeTruthy();
  });

  it("shows unpriced-model warning in money mode", () => {
    renderDialog(
      baseStatus({
        enabled: true,
        limitType: "money",
        currency: "USD",
        limitAmount: "10",
        unpricedRequestCount: 3,
        unpricedModels: ["unknown-x"],
      }),
    );
    expect(screen.getByText("usageLimit.pricingUnavailable:3")).toBeTruthy();
  });
});

describe("UsageLimitButton", () => {
  function renderButton(status?: UsageLimitStatus, onOpen?: () => void) {
    // 显式传入 status 时同步到 mock（与 hook mock 的全局返回一致）
    if (status !== undefined) {
      mockStatus = status;
    }
    return render(
      <UsageLimitButton
        providerId="p1"
        appId="claude"
        onOpen={onOpen ?? (() => {})}
      />,
    );
  }

  beforeEach(() => {
    mockStatus = undefined;
  });

  // Radix Tooltip 在 jsdom 依赖 floating-ui 定位，无法可靠触发渲染；
  // 项目测试惯例是断言四态样式与 aria-label（tooltip 文案与 Dialog
  // 启用行副标题共用 budgetSummary，由 Dialog 用例覆盖文本内容）。

  it("OFF state uses muted styling", () => {
    renderButton(baseStatus());
    const btn = screen.getByRole("button", { name: "usageLimit.title" });
    expect(btn.className).toContain("text-muted-foreground");
  });

  it("ACTIVE state keeps summary styling", () => {
    mockStatus = baseStatus({
      enabled: true,
      limitType: "money",
      currency: "USD",
      limitAmount: "10",
      usedMoneyUsd: "2.41",
      state: "active",
    });
    renderButton(mockStatus);
    expect(screen.getByRole("button").className).toContain("text-emerald-600");
  });

  it("WARNING state gets amber styling", () => {
    mockStatus = baseStatus({
      enabled: true,
      limitType: "token",
      limitAmount: "5000000",
      usedTokens: 4200000,
      state: "warning",
    });
    renderButton(mockStatus);
    const btn = screen.getByRole("button");
    expect(btn.className).toContain("text-amber-600");
  });

  it("EXHAUSTED state gets red styling", () => {
    mockStatus = baseStatus({
      enabled: true,
      limitType: "money",
      currency: "USD",
      limitAmount: "10",
      usedMoneyUsd: "10.42",
      state: "exhausted",
    });
    renderButton(mockStatus);
    const btn = screen.getByRole("button");
    expect(btn.className).toContain("text-red-600");
  });

  it("does not render for non-proxy apps", () => {
    const { container } = render(
      <UsageLimitButton providerId="p1" appId="opencode" onOpen={() => {}} />,
    );
    expect(container.firstElementChild).toBeNull();
  });

  it("opens the dialog on click", async () => {
    const onOpen = vi.fn();
    renderButton(baseStatus(), onOpen);
    fireEvent.click(screen.getByRole("button"));
    expect(onOpen).toHaveBeenCalled();
  });
});

describe("format helpers", () => {
  it("formats tokens compactly", () => {
    expect(formatTokensCompact(532)).toBe("532");
    expect(formatTokensCompact(12400)).toBe("12.4K");
    expect(formatTokensCompact(1250000)).toBe("1.25M");
    expect(formatTokensCompact(18200000)).toBe("18.2M");
  });

  it("formats money with currency symbols", () => {
    expect(formatMoney("12.34", "USD")).toBe("$12.34");
    expect(formatMoney("88.5", "CNY")).toBe("¥88.50");
  });
});
