import type { ValueOf } from "../internal/guards";

export const ADAPTIVE_REASONING_EFFORT = "auto";

export const PINNED_REASONING_EFFORTS = [
  "xhigh",
  "high",
  "medium",
  "low",
  "minimal",
  "none",
] as const;

export type PinnedReasoningEffort = ValueOf<typeof PINNED_REASONING_EFFORTS>;

export const REASONING_EFFORTS = [
  ...PINNED_REASONING_EFFORTS,
  ADAPTIVE_REASONING_EFFORT,
] as const;

export type ReasoningEffort = ValueOf<typeof REASONING_EFFORTS>;

export const DEFAULT_REASONING_EFFORT: ReasoningEffort = "high";

export const ADAPTIVE_REASONING_EFFORT_MODELS = ["openrouter/jev"] as const;

export function supportsAdaptiveReasoningEffort(model: string): boolean {
  const baseModel = model.split(":")[0];
  return ADAPTIVE_REASONING_EFFORT_MODELS.some((slug) => slug === baseModel);
}

export function defaultReasoningEffortFor(
  model: string | undefined
): ReasoningEffort {
  return model !== undefined && supportsAdaptiveReasoningEffort(model)
    ? ADAPTIVE_REASONING_EFFORT
    : DEFAULT_REASONING_EFFORT;
}

export function reasoningRequestFor(
  effort: ReasoningEffort
): { readonly effort: PinnedReasoningEffort } | undefined {
  return effort === ADAPTIVE_REASONING_EFFORT ? undefined : { effort };
}

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

export const VideoProcessingMode = {
  Agentic: "agentic",
  Static: "static",
} as const;

export const VIDEO_PROCESSING_MODES = [
  VideoProcessingMode.Agentic,
  VideoProcessingMode.Static,
] as const;

export type VideoProcessingMode = ValueOf<typeof VIDEO_PROCESSING_MODES>;
