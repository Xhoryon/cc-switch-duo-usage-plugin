// API Key 使用限额（Budget Limit）类型定义
//
// 与后端 `services/usage_limit.rs` 的 serde 输出一一对应（camelCase）。

/** 限额模式：同一时间只有一种 */
export type UsageLimitType = "money" | "token";

/** 金额模式币种。数据库内部成本统一为 USD，CNY 通过本地汇率换算 */
export type UsageLimitCurrency = "USD" | "CNY";

/** enforcement 能力：Proxy 是否真的在该 Provider 的请求路径上 */
export type UsageLimitEnforcement =
  | "active" // Proxy 已接管该应用，限额可强制执行
  | "proxy_disabled" // 该应用 Proxy 接管未开启，流量不经过 CC Switch
  | "unsupported_credential"; // OAuth 类 Provider，无静态 Key

/** 卡片图标 / Dialog 共用的状态聚合 */
export interface UsageLimitStatus {
  providerId: string;
  appType: string;
  enabled: boolean;
  limitType?: UsageLimitType | null;
  currency?: UsageLimitCurrency | null;
  limitAmount?: string | null;
  /** 预算统计窗口起点（unix 秒） */
  usageStartAt?: number | null;
  /** 自动重置周期（秒）；null 表示永不自动重置 */
  resetIntervalSeconds?: number | null;
  /** 窗口内已用金额（USD 原值，十进制字符串） */
  usedMoneyUsd: string;
  /** 限额币种下的已用金额（CNY 限额时已按汇率换算） */
  usedMoneyInCurrency?: string | null;
  /** 窗口内已用 token（与 Usage Dashboard 一致的 normalized total） */
  usedTokens: number;
  /** 使用百分比字符串（十进制，不 clamp，可超过 100） */
  percentUsed?: string | null;
  /** "off" | "active" | "warning" | "exhausted" */
  state: UsageLimitState;
  /** 窗口内「有 token 用量但当前无定价」的请求数（金额模式可靠性提示） */
  unpricedRequestCount: number;
  /** 上述请求涉及的模型列表（引导配置 custom pricing） */
  unpricedModels: string[];
  enforcement: UsageLimitEnforcement;
  /** 当前 credential 遮蔽展示（如 sk-****ABCD），无静态 key 时为 null */
  maskedCredential?: string | null;
}

export type UsageLimitState = "off" | "active" | "warning" | "exhausted";

/** 保存/开关提交载荷 */
export interface UsageLimitConfig {
  enabled: boolean;
  limitType: UsageLimitType;
  /** token 模式下为 null */
  currency?: UsageLimitCurrency | null;
  /** 金额（十进制字符串）或 token 数（正整数字符串） */
  limitAmount?: string | null;
  /** 自动重置周期（秒）；null 表示永不自动重置 */
  resetIntervalSeconds?: number | null;
}
