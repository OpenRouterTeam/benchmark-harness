import type { ValueOf } from "../../internal/guards";
import { z } from "../../internal/zod";
import { ORI_AGENTS } from "../agent-cli/schema";

export const TERMINAL_BENCH_4_VERSION = "4.0.0" as const;

export const TERMINAL_BENCH_4_AGENTS = ORI_AGENTS;

export type TerminalBench4Agent = ValueOf<typeof TERMINAL_BENCH_4_AGENTS>;

export const DEFAULT_TERMINAL_BENCH_4_AGENT: TerminalBench4Agent = "pi";

export const ArtifactSchema = z.union([
  z.string().min(1),
  z.object({
    source: z.string().min(1),
    exclude: z.array(z.string()).default([]),
    service: z.string().optional(),
  }),
]);

export type Artifact = z.infer<typeof ArtifactSchema>;

const EnvironmentSchema = z.object({
  cpus: z.number().int().positive(),
  memory_mb: z.number().int().positive(),
  storage_mb: z.number().int().positive(),
  gpus: z.number().int().nonnegative().default(0),
  gpu_types: z.array(z.string().min(1)).default([]),
  allow_internet: z.boolean().default(true),
  env: z.record(z.string(), z.string()).default({}),
});

export type TaskEnvironment = z.infer<typeof EnvironmentSchema>;

export const CollectHookSchema = z.object({
  command: z.string().min(1),
  service: z.string().optional(),
  timeout_sec: z.number().positive().optional(),
});

export type CollectHook = z.infer<typeof CollectHookSchema>;

export const TaskTomlSchema = z.object({
  artifacts: z.array(ArtifactSchema).default([]),
  task: z.object({ name: z.string().min(1) }),
  metadata: z.object({ category: z.string().min(1) }),
  agent: z.object({ timeout_sec: z.number().positive() }),
  verifier: z.object({
    timeout_sec: z.number().positive(),
    environment_mode: z.literal("separate"),
    environment: EnvironmentSchema.optional(),
    collect: z.array(CollectHookSchema).default([]),
    env: z.record(z.string(), z.string()).default({}),
  }),
  environment: EnvironmentSchema,
});

export type TaskToml = z.infer<typeof TaskTomlSchema>;

export interface TerminalBench4Task {
  readonly id: string;
  readonly taskToml: TaskToml;
  readonly taskDir: string;
  readonly instructionPath: string;
  readonly composeFile: string | undefined;
  readonly imageUser: string | undefined;
}
