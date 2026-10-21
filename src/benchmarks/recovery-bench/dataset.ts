import { readFileSync } from "node:fs";
import { join } from "node:path";

import { fromIterable } from "effect/Chunk";
import type { Effect } from "effect/Effect";
import { all, fail, map, mapError, succeed, suspend } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect } from "effect/Layer";
import { none, some } from "effect/Option";
import type { Stream } from "effect/Stream";
import {
  flatMap as flatMapStream,
  fromEffect,
  paginateChunkEffect,
} from "effect/Stream";
import { parse as tomlParse } from "smol-toml";

import type { Sample } from "../../harness/core";
import { DatasetError } from "../../harness/core";
import type {
  DatasetService,
  DatasetStreamOptions,
} from "../../harness/dataset";
import { Dataset } from "../../harness/dataset";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import { firstZodIssueMessage, parseSchema, z } from "../../internal/zod";
import type { RecoveryBenchManifest, RecoveryBenchTrial } from "./manifest";
import { RECOVERY_BENCH_MANIFEST, trajectoryUrl } from "./manifest";
import type { TerminalBench2TaskToml } from "./schema";
import {
  MAX_RECOVERY_INSTRUCTION_BYTES,
  TerminalBench2TaskTomlSchema,
} from "./schema";
import {
  ensureTasksCheckedOutEffect,
  RECOVERY_BENCH_SOURCE_COMMIT,
  TERMINAL_BENCH_2_SOURCE_COMMIT,
} from "./tasks-source";

export const RECOVERY_BENCH_DATASET_ID = "recovery_bench" as const;

const RecoveryBenchSampleMetaSchema = z.object({
  taskId: z.string().min(1),
  trialDir: z.string().min(1),
  trajectoryUrl: z.url(),
  trajectorySha256: z.string().length(64),
  trajectoryBytes: z.number().int().positive(),
  recoveryBenchCommit: z.string().length(40),
  terminalBenchCommit: z.string().length(40),
  dockerImage: z.string().min(1),
  maxAgentTimeoutSec: z.number().positive(),
  maxTestTimeoutSec: z.number().positive(),
  difficulty: z.enum(["easy", "medium", "hard"]),
  category: z.string(),
  cpus: z.number().int().positive(),
  memoryMb: z.number().int().positive(),
  gpus: z.number().int().nonnegative(),
  allowInternet: z.boolean(),
});

export type RecoveryBenchSampleMeta = z.infer<
  typeof RecoveryBenchSampleMetaSchema
>;

export function readRecoveryBenchMeta(
  metadata: unknown
): RecoveryBenchSampleMeta | undefined {
  if (!isRecord(metadata)) {
    return undefined;
  }
  const parsed = parseSchema(RecoveryBenchSampleMetaSchema, metadata);
  return Either.isRight(parsed) ? parsed.right : undefined;
}

export interface RecoveryBenchDatasetOptions {
  readonly taskSubset?: readonly string[];
  readonly maxAgentTimeoutSec?: number;
  readonly manifest?: RecoveryBenchManifest;
  readonly pageSize?: number;
  readonly maxInstructionBytes?: number;
}

interface LoadedTask {
  readonly taskToml: TerminalBench2TaskToml;
  readonly instruction: string;
}

function loadTask(
  tasksDir: string,
  taskId: string
): Either.Either<LoadedTask, DatasetError> {
  const taskDir = join(tasksDir, taskId);
  const tomlText = Either.try(() =>
    readFileSync(join(taskDir, "task.toml"), "utf8")
  );
  if (Either.isLeft(tomlText)) {
    return Either.left(
      new DatasetError({
        message: `recovery-bench task "${taskId}" is missing task.toml in the Terminal-Bench 2.0 checkout: ${String(tomlText.left)}`,
      })
    );
  }
  const raw = Either.try((): unknown => tomlParse(tomlText.right));
  if (Either.isLeft(raw)) {
    return Either.left(
      new DatasetError({
        message: `recovery-bench task "${taskId}" has unparseable task.toml: ${String(raw.left)}`,
      })
    );
  }
  const parsed = parseSchema(TerminalBench2TaskTomlSchema, raw.right);
  if (Either.isLeft(parsed)) {
    return Either.left(
      new DatasetError({
        message: `recovery-bench task "${taskId}" has invalid task.toml: ${firstZodIssueMessage(parsed.left)}`,
      })
    );
  }
  const instruction = Either.try(() =>
    readFileSync(join(taskDir, "instruction.md"), "utf8")
  );
  if (Either.isLeft(instruction)) {
    return Either.left(
      new DatasetError({
        message: `recovery-bench task "${taskId}" is missing instruction.md: ${String(instruction.left)}`,
      })
    );
  }
  return Either.right({
    taskToml: parsed.right,
    instruction: instruction.right,
  });
}

export function recoveryBenchSampleId(trial: RecoveryBenchTrial): string {
  return `${RECOVERY_BENCH_DATASET_ID}-${trial.trialDir}`;
}

function toSample(
  manifest: RecoveryBenchManifest,
  trial: RecoveryBenchTrial,
  task: LoadedTask,
  maxAgentTimeoutSecOverride: number | undefined
): Sample {
  const { environment, agent, verifier, metadata } = task.taskToml;
  const meta: RecoveryBenchSampleMeta = {
    taskId: trial.taskId,
    trialDir: trial.trialDir,
    trajectoryUrl: trajectoryUrl(manifest, trial),
    trajectorySha256: trial.trajectorySha256,
    trajectoryBytes: trial.trajectoryBytes,
    recoveryBenchCommit: manifest.sourceCommit,
    terminalBenchCommit: TERMINAL_BENCH_2_SOURCE_COMMIT,
    dockerImage: environment.docker_image,
    maxAgentTimeoutSec: maxAgentTimeoutSecOverride ?? agent.timeout_sec,
    maxTestTimeoutSec: verifier.timeout_sec,
    difficulty: metadata.difficulty,
    category: metadata.category,
    cpus: environment.cpus,
    memoryMb: environment.memory,
    gpus: environment.gpus,
    allowInternet: environment.allow_internet,
  };
  return {
    id: recoveryBenchSampleId(trial),
    input: task.instruction,
    target: { text: trial.taskId },
    metadata: meta,
  };
}

export function selectTrials(
  manifest: RecoveryBenchManifest,
  taskSubset: readonly string[] | undefined,
  maxInstructionBytes: number = MAX_RECOVERY_INSTRUCTION_BYTES
): Either.Either<readonly RecoveryBenchTrial[], DatasetError> {
  const fitting = manifest.trials.filter(
    (trial) => trial.fullContextBytes <= maxInstructionBytes
  );
  if (taskSubset === undefined) {
    return Either.right(fitting);
  }
  const wanted = new Set(taskSubset);
  const selected = fitting.filter(
    (trial) => wanted.has(trial.taskId) || wanted.has(trial.trialDir)
  );
  const found = new Set(
    selected.flatMap((trial) => [trial.taskId, trial.trialDir])
  );
  const missing = taskSubset.filter((id) => !found.has(id));
  if (missing.length > 0) {
    return Either.left(
      new DatasetError({
        message: `recovery-bench taskSubset contains unknown entries: ${missing.join(", ")}`,
      })
    );
  }
  return Either.right(selected);
}

export function makeRecoveryBenchDataset(
  opts: RecoveryBenchDatasetOptions = {}
): DatasetService {
  const manifest = opts.manifest ?? RECOVERY_BENCH_MANIFEST;
  const pageSize = opts.pageSize ?? DEFAULT_PAGE_SIZE;
  const selectedTrials: Effect<readonly RecoveryBenchTrial[], DatasetError> =
    manifest.sourceCommit === RECOVERY_BENCH_SOURCE_COMMIT
      ? suspend(() => {
          const selected = selectTrials(
            manifest,
            opts.taskSubset,
            opts.maxInstructionBytes
          );
          return Either.isLeft(selected)
            ? fail(selected.left)
            : succeed(selected.right);
        })
      : fail(
          new DatasetError({
            message: `recovery-bench manifest commit ${manifest.sourceCommit} does not match pinned ${RECOVERY_BENCH_SOURCE_COMMIT}`,
          })
        );
  const tasksDirEffect = ensureTasksCheckedOutEffect().pipe(
    mapError((e) => new DatasetError({ message: e.message }))
  );
  const size = selectedTrials.pipe(map((trials) => trials.length));
  const stream = (
    streamOpts?: DatasetStreamOptions
  ): Stream<Sample, DatasetError> =>
    fromEffect(all([selectedTrials, tasksDirEffect])).pipe(
      flatMapStream(([trials, tasksDir]) => {
        const start = streamOpts?.start ?? 0;
        const end = Math.min(streamOpts?.end ?? trials.length, trials.length);
        const window = trials.slice(start, end);
        return paginateChunkEffect(0, (offset: number) => {
          const page = window.slice(offset, offset + pageSize);
          const samples: Sample[] = [];
          for (const trial of page) {
            const task = loadTask(tasksDir, trial.taskId);
            if (Either.isLeft(task)) {
              return fail(task.left);
            }
            samples.push(
              toSample(manifest, trial, task.right, opts.maxAgentTimeoutSec)
            );
          }
          const nextOffset = offset + pageSize;
          return succeed([
            fromIterable(samples),
            nextOffset < window.length ? some(nextOffset) : none(),
          ] as const);
        });
      })
    );
  return { stream, size };
}

const DEFAULT_PAGE_SIZE = 8;

export function makeRecoveryBenchDatasetLayer(
  opts: RecoveryBenchDatasetOptions = {}
): Layer<Dataset> {
  return effect(Dataset, succeed(Dataset.of(makeRecoveryBenchDataset(opts))));
}
