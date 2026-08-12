import type { ValueOf } from "../../internal/guards";

export const ORI_AGENTS = ["claude"] as const;

export type OriAgent = ValueOf<typeof ORI_AGENTS>;

export const HARBOR_AGENTS = ["native", "claude"] as const;

export type HarborAgent = ValueOf<typeof HARBOR_AGENTS>;

export const DEFAULT_HARBOR_AGENT: HarborAgent = "native";

export const CLAUDE_EFFORT_LEVELS = [
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
] as const;

export type ClaudeEffortLevel = ValueOf<typeof CLAUDE_EFFORT_LEVELS>;

export const DEFAULT_CLAUDE_EFFORT: ClaudeEffortLevel = "medium";

export const DEFAULT_CLAUDE_PACKAGE =
  "@anthropic-ai/claude-code@latest" as const;

export const DEFAULT_ORI_INSTALL_URL =
  "https://openrouter.ai/labs/ori/install.sh" as const;

export function isOriAgent(agent: HarborAgent): agent is OriAgent {
  return agent !== "native";
}
