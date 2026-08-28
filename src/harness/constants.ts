import type { ValueOf } from "../internal/guards";

export const REASONING_EFFORTS = [
  "xhigh",
  "high",
  "medium",
  "low",
  "minimal",
  "none",
] as const;

export type ReasoningEffort = ValueOf<typeof REASONING_EFFORTS>;

export const DEFAULT_REASONING_EFFORT: ReasoningEffort = "high";

export const COST_TIERS = ["low", "medium", "high", "xhigh", "max"] as const;

export type CostTier = ValueOf<typeof COST_TIERS>;

export const ImageDetail = {
  Auto: "auto",
  Low: "low",
  High: "high",
} as const;

export const IMAGE_DETAIL_VALUES = [
  ImageDetail.Auto,
  ImageDetail.Low,
  ImageDetail.High,
] as const;

export type ImageDetail = ValueOf<typeof IMAGE_DETAIL_VALUES>;
