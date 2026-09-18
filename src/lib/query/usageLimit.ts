import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { usageLimitApi } from "@/lib/api/usageLimit";
import type { AppId } from "@/lib/api/types";
import type { UsageLimitConfig } from "@/types/usageLimit";

/** Query keys（独立命名空间，避免与 Usage Dashboard 的轮询互相牵连） */
export const usageLimitKeys = {
  all: ["usage-limit"] as const,
  status: (providerId: string, appType: string) =>
    [...usageLimitKeys.all, "status", providerId, appType] as const,
  exchangeRate: () => [...usageLimitKeys.all, "usd-cny-rate"] as const,
};

/**
 * 读取某个 Provider 的限额状态。
 *
 * - `enabled` 由调用方控制（只对可走 Local Proxy 的应用启用）
 * - `refetchOnMount: "always"`：Dialog / 卡片重新挂载时拿到最新用量，
 *   请求完成后的刷新由 usage-log-recorded 事件的 invalidate 兜底
 */
export function useUsageLimitStatus(providerId: string, appType: AppId) {
  return useQuery({
    queryKey: usageLimitKeys.status(providerId, appType),
    queryFn: () => usageLimitApi.getStatus(providerId, appType),
    enabled: Boolean(providerId) && Boolean(appType),
    refetchOnMount: "always",
  });
}

/** 本地 USD → CNY 汇率 */
export function useUsdCnyRate() {
  return useQuery({
    queryKey: usageLimitKeys.exchangeRate(),
    queryFn: usageLimitApi.getUsdCnyRate,
    staleTime: 60_000,
  });
}

function useInvalidateUsageLimit(providerId: string, appType: AppId) {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({
      queryKey: usageLimitKeys.status(providerId, appType),
    });
  };
}

/** 保存限额配置（新建 / 修改 / 启停） */
export function useSaveUsageLimit(providerId: string, appType: AppId) {
  const invalidate = useInvalidateUsageLimit(providerId, appType);
  return useMutation({
    mutationFn: (config: UsageLimitConfig) =>
      usageLimitApi.save(providerId, appType, config),
    onSuccess: invalidate,
  });
}

/** 重置使用量：usage_start_at 推进到当前时间，不删除历史 Usage */
export function useResetUsageLimit(providerId: string, appType: AppId) {
  const invalidate = useInvalidateUsageLimit(providerId, appType);
  return useMutation({
    mutationFn: () => usageLimitApi.resetUsage(providerId, appType),
    onSuccess: invalidate,
  });
}

/** 保存本地汇率 */
export function useSetUsdCnyRate() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (rate: string) => usageLimitApi.setUsdCnyRate(rate),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: usageLimitKeys.exchangeRate(),
      });
      // 已换算的 CNY 用量展示依赖汇率，一并刷新
      void queryClient.invalidateQueries({ queryKey: usageLimitKeys.all });
    },
  });
}
