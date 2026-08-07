import type { ValueOf } from "../../internal/guards";
import { z } from "../../internal/zod";

export const AGENT_DX_SUITES = [
  "benchmark",
  "regression",
  "discoverability",
] as const;
export type AgentDxSuite = ValueOf<typeof AGENT_DX_SUITES>;
export const DEFAULT_AGENT_DX_SUITE: AgentDxSuite = "benchmark";

export const AgentDxTaskTomlSchema = z.object({
  schema_version: z.string(),
  task: z.object({
    name: z.string(),
    description: z.string(),
    keywords: z.array(z.string()).default([]),
    disabled: z.boolean().default(false),
    suite: z.enum(AGENT_DX_SUITES).default(DEFAULT_AGENT_DX_SUITE),
  }),
  metadata: z.object({
    difficulty: z.enum(["easy", "medium", "hard"]),
    category: z.string(),
    alignment_criteria: z.array(z.string()).default([]),
  }),
  agent: z.object({ timeout_sec: z.number().positive() }),
  verifier: z.object({ timeout_sec: z.number().positive() }),
  environment: z.object({
    docker_image: z.string(),
    env: z.record(z.string(), z.string()).default({}),
  }),
});
export type AgentDxTaskToml = z.infer<typeof AgentDxTaskTomlSchema>;

export interface AgentDxTask {
  readonly id: string;
  readonly taskToml: AgentDxTaskToml;
  readonly taskDir: string;
  readonly testDir: string;
  readonly testScript: string;
  readonly instructionPath: string;
  readonly dockerImage: string;
}

export const AGENT_DX_HARNESSES = ["opencode", "claude-code"] as const;
export type AgentDxHarness = ValueOf<typeof AGENT_DX_HARNESSES>;

export const AGENT_DX_PROFILES = [
  "baseline",
  "docs",
  "skills",
  "mcp",
  "agents",
] as const;
export type AgentDxProfile = ValueOf<typeof AGENT_DX_PROFILES>;

export const DEFAULT_AGENT_DX_HARNESS: AgentDxHarness = "opencode";
export const DEFAULT_AGENT_DX_PROFILE: AgentDxProfile = "baseline";

export const AGENT_DX_SANDBOX_KEY_MODES = ["provided", "absent"] as const;
export type AgentDxSandboxKeyMode = ValueOf<typeof AGENT_DX_SANDBOX_KEY_MODES>;
export const DEFAULT_AGENT_DX_SANDBOX_KEY_MODE: AgentDxSandboxKeyMode =
  "provided";
export const DEFAULT_OPENCODE_PACKAGE = "opencode-ai@1.18.11" as const;
export const CLAUDE_CODE_PACKAGE = "@anthropic-ai/claude-code@2.1.220" as const;
export const OPENROUTER_MCP_URL = "https://mcp.openrouter.ai/mcp" as const;
export const DEFAULT_AGENT_DX_SKILLS_SOURCE =
  "https://github.com/OpenRouterTeam/skills" as const;

export const AGENT_DX_SKILLS_SOURCE_PATTERN =
  /^https:\/\/[\w.-]+(?:\/[\w.-]+)*(?:#[\w.][\w./-]*)?$/;

export const AGENT_DX_OPENCODE_PACKAGE_PATTERN =
  /^(?:@[\w.-]+\/)?[\w.-]+(?:@[\w.-]+)?$/;

export const DEFAULT_AGENT_DX_DOCS_SOURCE =
  "https://openrouter.ai/docs/llms-full.txt" as const;

export const AGENT_DX_DOCS_SNAPSHOT_PATH =
  "/opt/openrouter-docs/llms-full.txt" as const;

export const REMOTE_AGENT_LOG = "/logs/agent/agent.jsonl" as const;
export const REMOTE_AGENT_STDERR_LOG = "/logs/agent/agent-stderr.log" as const;

export const AGENT_DX_DOCS_ADDENDUM_PATH =
  "/opt/openrouter-docs/addendum.md" as const;

export const AGENT_DX_DOCS_SOURCE_PATTERN = /^https:\/\/[\w.-]+(?:\/[\w.-]+)*$/;
