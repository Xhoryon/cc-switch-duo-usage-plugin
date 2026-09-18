import { useEffect } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useQueryClient } from "@tanstack/react-query";
import { usageKeys } from "@/lib/query/usage";
import { usageLimitKeys } from "@/lib/query/usageLimit";

/**
 * 监听后端 `usage-log-recorded` 事件，收到后立刻 invalidate 所有
 * UsageDashboard 相关查询，让用户无需等待 30s 轮询周期。
 *
 * 后端在 `proxy_request_logs` 写入新行时会 emit 该事件（200ms 防抖合并），
 * 来源覆盖代理日志、Claude/Codex/Gemini 会话同步、启动归档。
 *
 * 使用限额（Budget）状态共享同一 SSOT：请求记账后其用量/进度也要跟着
 * 刷新，因此这里一并 invalidate usage-limit 命名空间。
 *
 * 该 hook 只挂在 UsageDashboard 上，避免在主界面其他位置无意义触发。
 */
export function useUsageEventBridge() {
  const queryClient = useQueryClient();

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;

    (async () => {
      const off = await listen("usage-log-recorded", () => {
        // invalidate 整个 usage 命名空间：summary / trends / providerStats /
        // modelStats / logs 全部跟着重拉
        queryClient.invalidateQueries({ queryKey: usageKeys.all });
        // 使用限额与 Usage Dashboard 同源（proxy_request_logs），同步刷新
        queryClient.invalidateQueries({ queryKey: usageLimitKeys.all });
      });

      if (disposed) {
        off();
      } else {
        unlisten = off;
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [queryClient]);
}

/**
 * 供主界面供应商列表使用的轻量版本：只 invalidate usage-limit 命名空间，
 * 让 Provider Card 的限额图标与 Dialog 在每次请求记账后拿到最新用量。
 * Usage Dashboard 不挂载时（用户停留在主界面）也要能刷新限额状态。
 */
export function useUsageLimitEventBridge() {
  const queryClient = useQueryClient();

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;

    (async () => {
      const off = await listen("usage-log-recorded", () => {
        queryClient.invalidateQueries({ queryKey: usageLimitKeys.all });
      });

      if (disposed) {
        off();
      } else {
        unlisten = off;
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [queryClient]);
}
