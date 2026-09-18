import { invoke } from "@tauri-apps/api/core";
import type {
  UsageLimitConfig,
  UsageLimitCurrency,
  UsageLimitStatus,
} from "@/types/usageLimit";
import type { AppId } from "./types";

export const usageLimitApi = {
  getStatus: async (
    providerId: string,
    appId: AppId,
  ): Promise<UsageLimitStatus> => {
    return invoke("get_usage_limit_status", {
      providerId,
      appType: appId,
    });
  },

  save: async (
    providerId: string,
    appId: AppId,
    config: UsageLimitConfig,
  ): Promise<UsageLimitStatus> => {
    return invoke("save_usage_limit", {
      providerId,
      appType: appId,
      config,
    });
  },

  resetUsage: async (
    providerId: string,
    appId: AppId,
  ): Promise<UsageLimitStatus> => {
    return invoke("reset_usage_limit", { providerId, appType: appId });
  },

  getUsdCnyRate: async (): Promise<string> => {
    return invoke("get_usd_cny_rate");
  },

  setUsdCnyRate: async (rate: string): Promise<void> => {
    return invoke("set_usd_cny_rate", { rate });
  },
};

export type { UsageLimitCurrency };
