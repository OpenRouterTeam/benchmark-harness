import type { ValueOf } from "../../internal/guards";
import { z } from "../../internal/zod";

export const TaskTomlSchema = z.object({
  schema_version: z.string(),
  task: z.object({
    name: z.string(),
    description: z.string(),
    keywords: z.array(z.string()).default([]),
  }),
  metadata: z.object({
    author_name: z.string(),
    author_email: z.string(),
    difficulty: z.enum(["easy", "medium", "hard"]),
    category: z.string(),
    tags: z.array(z.string()).default([]),
    expert_time_estimate_min: z.number().optional(),
    junior_time_estimate_min: z.number().optional(),
  }),
  agent: z.object({ timeout_sec: z.number().positive() }),
  verifier: z.object({
    timeout_sec: z.number().positive(),
    env: z.record(z.string(), z.string()).optional(),
  }),
  environment: z.object({
    build_timeout_sec: z.number().positive().optional(),
    docker_image: z.string(),
    cpus: z.number().int().positive(),
    memory_mb: z.number().int().positive(),
    storage_mb: z.number().int().positive().optional(),
    gpus: z.number().int().nonnegative(),
    allow_internet: z.boolean().default(true),
    mcp_servers: z.array(z.unknown()).default([]),
    env: z.record(z.string(), z.string()).optional(),
  }),
  solution: z
    .object({ env: z.record(z.string(), z.string()).optional() })
    .optional(),
});

export type TaskToml = z.infer<typeof TaskTomlSchema>;

export interface TerminalBenchTask {
  readonly id: string;
  readonly taskToml: TaskToml;
  readonly taskDir: string;
  readonly testDir: string;
  readonly testScript: string;
  readonly instructionPath: string;
  readonly dockerImage: string;
}

export const TERMINAL_BENCH_VERSION = "2.1" as const;

export const PI_THINKING_LEVELS = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
] as const;

export type PiThinkingLevel = ValueOf<typeof PI_THINKING_LEVELS>;

export const DEFAULT_PI_THINKING: PiThinkingLevel = "medium";

export const CLAUDE_EFFORT_LEVELS = [
  "low",
  "medium",
  "high",
  "xhigh",
  "max",
] as const;

export type ClaudeEffortLevel = ValueOf<typeof CLAUDE_EFFORT_LEVELS>;

export const DEFAULT_CLAUDE_EFFORT: ClaudeEffortLevel = "medium";

export const DEFAULT_PI_PACKAGE =
  "@earendil-works/pi-coding-agent@latest" as const;

export const TERMINAL_BENCH_AGENTS = ["pi", "claude"] as const;

export type TerminalBenchAgent = ValueOf<typeof TERMINAL_BENCH_AGENTS>;

export const DEFAULT_TERMINAL_BENCH_AGENT: TerminalBenchAgent = "pi";

export const ORI_AGENTS = ["claude"] as const;

export type OriAgent = ValueOf<typeof ORI_AGENTS>;

export const DEFAULT_ORI_INSTALL_URL =
  "https://openrouter.ai/labs/ori/install.sh" as const;

export const DEFAULT_CLAUDE_PACKAGE =
  "@anthropic-ai/claude-code@latest" as const;
