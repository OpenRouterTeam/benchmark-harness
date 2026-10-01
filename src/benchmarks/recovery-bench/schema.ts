import type { ValueOf } from "../../internal/guards";
import { z } from "../../internal/zod";

export const RECOVERY_BENCH_MESSAGE_MODES = [
  "full",
  "summary",
  "none",
] as const;

export type RecoveryBenchMessageMode = ValueOf<
  typeof RECOVERY_BENCH_MESSAGE_MODES
>;

export const DEFAULT_RECOVERY_BENCH_MESSAGE_MODE: RecoveryBenchMessageMode =
  "full";

export const DEFAULT_REPLAY_COMMAND_TIMEOUT_SEC = 15;

export const MAX_RECOVERY_INSTRUCTION_BYTES = 64000;

const MEMORY_PATTERN = /^(\d+)([KMGT])$/i;

const MEMORY_UNIT_MB: Readonly<Record<string, number>> = {
  K: 1 / 1024,
  M: 1,
  G: 1024,
  T: 1024 * 1024,
};

export function parseMemoryMb(value: string): number | undefined {
  const match = MEMORY_PATTERN.exec(value.trim());
  if (match === null) {
    return undefined;
  }
  const amount = Number(match[1]);
  const unit = (match[2] ?? "").toUpperCase();
  const factor = MEMORY_UNIT_MB[unit];
  if (factor === undefined) {
    return undefined;
  }
  const mb = Math.round(amount * factor);
  return mb > 0 ? mb : undefined;
}

const MemoryMbSchema = z.string().transform((value, ctx) => {
  const mb = parseMemoryMb(value);
  if (mb === undefined) {
    ctx.addIssue({
      code: "custom",
      message: `unrecognized memory size "${value}"`,
    });
    return z.NEVER;
  }
  return mb;
});

export const TerminalBench2TaskTomlSchema = z.object({
  version: z.string(),
  metadata: z.object({
    difficulty: z.enum(["easy", "medium", "hard"]),
    category: z.string(),
    tags: z.array(z.string()).default([]),
  }),
  agent: z.object({ timeout_sec: z.number().positive() }),
  verifier: z.object({ timeout_sec: z.number().positive() }),
  environment: z.object({
    build_timeout_sec: z.number().positive().optional(),
    docker_image: z.string().min(1),
    cpus: z.number().int().positive(),
    memory: MemoryMbSchema,
    gpus: z.number().int().nonnegative().default(0),
    allow_internet: z.boolean().default(true),
  }),
});

export type TerminalBench2TaskToml = z.infer<
  typeof TerminalBench2TaskTomlSchema
>;

const ToolCallSchema = z.object({
  arguments: z
    .object({
      keystrokes: z.string().optional(),
      duration: z.number().optional(),
    })
    .loose()
    .optional(),
});

const StepSchema = z
  .object({
    source: z.enum(["system", "user", "agent"]),
    message: z.string().default(""),
    tool_calls: z.array(ToolCallSchema).optional(),
  })
  .loose();

export const AtifTrajectorySchema = z
  .object({
    schema_version: z.string(),
    steps: z.array(StepSchema),
    agent: z.object({ model_name: z.string().optional() }).loose().optional(),
  })
  .loose();

export type AtifTrajectory = z.infer<typeof AtifTrajectorySchema>;

export type AtifStep = z.infer<typeof StepSchema>;
