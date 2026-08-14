import type { ValueOf } from "../../internal/guards";

export const ORI_AGENTS = ["pi", "claude"] as const;

export type OriAgent = ValueOf<typeof ORI_AGENTS>;

export const HARBOR_AGENTS = ["native", "pi", "claude"] as const;

export type HarborAgent = ValueOf<typeof HARBOR_AGENTS>;

export const DEFAULT_HARBOR_AGENT: HarborAgent = "native";

export const ORI_REASONING_EFFORTS = [
  "none",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
] as const;

export type OriReasoningEffort = ValueOf<typeof ORI_REASONING_EFFORTS>;

export const DEFAULT_ORI_REASONING_EFFORT: OriReasoningEffort = "medium";

export const DEFAULT_CLAUDE_PACKAGE =
  "@anthropic-ai/claude-code@latest" as const;

export const DEFAULT_ORI_INSTALL_URL =
  "https://openrouter.ai/labs/ori/install.sh" as const;

export const ORI_CHANNELS = ["stable", "alpha"] as const;

export type OriChannel = ValueOf<typeof ORI_CHANNELS>;

export const DEFAULT_ORI_CHANNEL: OriChannel = "stable";

export function isOriAgent(agent: HarborAgent): agent is OriAgent {
  return agent !== "native";
}
