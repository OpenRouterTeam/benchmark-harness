import { Either } from "../../internal/either";
import { firstZodIssueMessage, parseSchema, z } from "../../internal/zod";
import manifestJson from "./recovery-bench-manifest.json";

const RAW_MEDIA_BASE =
  "https://media.githubusercontent.com/media/letta-ai/recovery-bench" as const;

const ManifestTrialSchema = z.object({
  taskId: z.string().min(1),
  trialDir: z.string().min(1),
  trajectorySha256: z.string().length(64),
  trajectoryBytes: z.number().int().positive(),
  fullContextBytes: z.number().int().positive(),
  replayCommands: z.number().int().nonnegative(),
});

const ManifestSchema = z.object({
  sourceCommit: z.string().length(40),
  run: z.string().min(1),
  trials: z.array(ManifestTrialSchema).min(1),
});

export type RecoveryBenchTrial = z.infer<typeof ManifestTrialSchema>;

export interface RecoveryBenchManifest {
  readonly sourceCommit: string;
  readonly run: string;
  readonly trials: readonly RecoveryBenchTrial[];
}

export function buildRecoveryBenchManifest(
  raw: unknown
): RecoveryBenchManifest {
  const parsed = parseSchema(ManifestSchema, raw);
  if (Either.isLeft(parsed)) {
    throw new TypeError(
      `recovery-bench manifest is invalid: ${firstZodIssueMessage(parsed.left)}`
    );
  }
  const seen = new Set<string>();
  for (const trial of parsed.right.trials) {
    if (seen.has(trial.trialDir)) {
      throw new TypeError(
        `recovery-bench manifest has duplicate trialDir "${trial.trialDir}"`
      );
    }
    seen.add(trial.trialDir);
  }
  return parsed.right;
}

export function trajectoryUrl(
  manifest: RecoveryBenchManifest,
  trial: RecoveryBenchTrial
): string {
  return `${RAW_MEDIA_BASE}/${manifest.sourceCommit}/runs/${manifest.run}/${trial.trialDir}/agent/trajectory.json`;
}

export const RECOVERY_BENCH_MANIFEST: RecoveryBenchManifest =
  buildRecoveryBenchManifest(manifestJson);
