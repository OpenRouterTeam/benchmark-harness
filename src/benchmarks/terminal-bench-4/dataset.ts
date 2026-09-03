import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

import { fromIterable } from "effect/Chunk";
import type { Effect } from "effect/Effect";
import { fail, flatMap, mapError, succeed } from "effect/Effect";
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
import type { DatasetStreamOptions } from "../../harness/dataset";
import { Dataset } from "../../harness/dataset";
import { Either } from "../../internal/either";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import type { TaskEnvironment, TerminalBench4Task } from "./schema";
import { ArtifactSchema, CollectHookSchema, TaskTomlSchema } from "./schema";
import { ensureTasksCheckedOutEffect, tasksDir } from "./tasks-source";

export const TERMINAL_BENCH_4_DATASET_ID = "terminal_bench_4" as const;

const COMPOSE_FILE = join("environment", "docker-compose.yaml");

export function loadTask(
  taskId: string,
  tasksDirPath: string
): TerminalBench4Task {
  const taskDir = join(tasksDirPath, taskId);
  const raw = readFileSync(join(taskDir, "task.toml"), "utf8");
  const tomlObj = Either.try(() => tomlParse(raw));
  if (Either.isLeft(tomlObj)) {
    throw new Error(
      `terminal-bench-4 task "${taskId}" task.toml failed to parse: ${String(tomlObj.left)}`
    );
  }
  const parsed = parseSchema(TaskTomlSchema, tomlObj.right);
  if (Either.isLeft(parsed)) {
    throw new Error(
      `terminal-bench-4 task "${taskId}" task.toml failed validation: ${parsed.left.message}`
    );
  }
  const composePath = join(taskDir, COMPOSE_FILE);
  return {
    id: taskId,
    taskToml: parsed.right,
    taskDir,
    instructionPath: join(taskDir, "instruction.md"),
    composeFile: existsSync(composePath) ? composePath : undefined,
  };
}

function isTaskDir(tasksDirPath: string, entry: string): boolean {
  if (entry.startsWith(".")) {
    return false;
  }
  try {
    return (
      statSync(join(tasksDirPath, entry)).isDirectory() &&
      existsSync(join(tasksDirPath, entry, "task.toml"))
    );
  } catch {
    return false;
  }
}

export function listComposeTaskIds(tasksDirPath: string): readonly string[] {
  return readdirSync(tasksDirPath)
    .filter(
      (entry) =>
        isTaskDir(tasksDirPath, entry) &&
        existsSync(join(tasksDirPath, entry, COMPOSE_FILE))
    )
    .sort();
}

export function listTaskIds(
  tasksDirPath: string,
  taskSubset?: readonly string[]
): readonly string[] {
  const onDisk = readdirSync(tasksDirPath).filter(
    (entry) =>
      isTaskDir(tasksDirPath, entry) &&
      !existsSync(join(tasksDirPath, entry, COMPOSE_FILE))
  );
  if (taskSubset !== undefined && taskSubset.length > 0) {
    const diskSet = new Set(onDisk);
    return taskSubset.filter((id) => diskSet.has(id));
  }
  return [...onDisk].sort();
}

const SandboxResourcesSchema = z.object({
  cpus: z.number().int().positive(),
  memoryMb: z.number().int().positive(),
  gpu: z.string().min(1).optional(),
  env: z.record(z.string(), z.string()),
  allowInternet: z.boolean(),
});

export type SandboxResources = z.infer<typeof SandboxResourcesSchema>;

export const TerminalBench4SampleMetaSchema = z.object({
  taskId: z.string().min(1),
  maxAgentTimeoutSec: z.number().positive(),
  maxTestTimeoutSec: z.number().positive(),
  category: z.string(),
  agentEnv: SandboxResourcesSchema,
  verifierEnv: SandboxResourcesSchema,
  artifacts: z.array(ArtifactSchema),
  collect: z.array(CollectHookSchema),
  reward: z.number().optional(),
  testOutput: z.string().optional(),
});

export type TerminalBench4SampleMeta = z.infer<
  typeof TerminalBench4SampleMetaSchema
>;

export function toSandboxResources(
  env: TaskEnvironment,
  extraEnvVars: Readonly<Record<string, string>> = {}
): SandboxResources {
  return definedValues({
    cpus: env.cpus,
    memoryMb: env.memory_mb,
    gpu: toModalGpu(env),
    env: { ...env.env, ...extraEnvVars },
    allowInternet: env.allow_internet,
  });
}

export function toModalGpu(env: TaskEnvironment): string | undefined {
  if (env.gpus === 0) {
    return undefined;
  }
  const type = env.gpu_types[0];
  if (type === undefined) {
    throw new Error(
      `task requests ${env.gpus} GPU(s) but declares no gpu_types; refusing to guess an accelerator`
    );
  }
  return env.gpus === 1 ? type : `${type}:${env.gpus}`;
}

export function taskToSample(
  task: TerminalBench4Task,
  maxAgentTimeoutSecOverride?: number
): Sample {
  if (task.composeFile !== undefined) {
    throw new Error(
      `terminal-bench-4 task "${task.id}" uses docker-compose, which this harness does not support`
    );
  }
  const instruction = readFileSync(task.instructionPath, "utf8");
  const { environment, verifier, agent, metadata, artifacts } = task.taskToml;
  const meta: TerminalBench4SampleMeta = {
    taskId: task.id,
    maxAgentTimeoutSec: maxAgentTimeoutSecOverride ?? agent.timeout_sec,
    maxTestTimeoutSec: verifier.timeout_sec,
    category: metadata.category,
    agentEnv: toSandboxResources(environment),
    verifierEnv: toSandboxResources(
      verifier.environment ?? environment,
      verifier.env
    ),
    artifacts,
    collect: verifier.collect,
  };
  return {
    id: `${TERMINAL_BENCH_4_DATASET_ID}-${task.id}`,
    input: instruction,
    target: { text: task.id },
    metadata: meta,
  };
}

export function readTerminalBench4Meta(
  metadata: Readonly<Record<string, unknown>> | undefined
): TerminalBench4SampleMeta | undefined {
  if (metadata === undefined) {
    return undefined;
  }
  const parsed = parseSchema(TerminalBench4SampleMetaSchema, metadata);
  return Either.isLeft(parsed) ? undefined : parsed.right;
}

export interface TerminalBench4DatasetConfig {
  readonly taskSubset?: readonly string[];
  readonly maxAgentTimeoutSec?: number;
  readonly pageSize?: number;
}

export function makeTerminalBench4DatasetLayer(
  config?: TerminalBench4DatasetConfig
): Layer<Dataset> {
  const pageSize = config?.pageSize ?? 20;
  const taskSubset = config?.taskSubset;
  const maxAgentTimeoutSec = config?.maxAgentTimeoutSec;
  return effect(
    Dataset,
    succeed(buildDatasetService({ pageSize, taskSubset, maxAgentTimeoutSec }))
  );
}

function buildDatasetService(opts: {
  readonly pageSize: number;
  readonly taskSubset?: readonly string[];
  readonly maxAgentTimeoutSec?: number;
}): ReturnType<typeof Dataset.of> {
  const { pageSize, taskSubset, maxAgentTimeoutSec } = opts;
  const tasksDirEffect = ensureTasksCheckedOutEffect().pipe(
    mapError((e) => new DatasetError({ message: e.message })),
    flatMap((root) => succeed(tasksDir(root)))
  );
  const sizeEffect: Effect<number, DatasetError> = tasksDirEffect.pipe(
    flatMap((dir) => succeed(listTaskIds(dir, taskSubset).length))
  );
  const stream = (opts2?: DatasetStreamOptions): Stream<Sample, DatasetError> =>
    fromEffect(tasksDirEffect).pipe(
      flatMapStream((dir: string) => {
        const allIds = listTaskIds(dir, taskSubset);
        const start = opts2?.start ?? 0;
        const requestedEnd = opts2?.end ?? allIds.length;
        const end = Math.min(requestedEnd, allIds.length);
        const pageIds = allIds.slice(start, end);
        return paginateChunkEffect(0, (offset: number) => {
          const pageSlice = pageIds.slice(offset, offset + pageSize);
          const built = Either.try(() =>
            pageSlice.map((id) =>
              taskToSample(loadTask(id, dir), maxAgentTimeoutSec)
            )
          );
          if (Either.isLeft(built)) {
            return fail(
              new DatasetError({
                message: `Failed to load terminal-bench-4 tasks at offset ${offset}: ${String(built.left)}`,
              })
            );
          }
          const nextOffset = offset + pageSize;
          const hasMore = nextOffset < pageIds.length;
          return succeed([
            fromIterable(built.right),
            hasMore ? some(nextOffset) : none(),
          ] as const);
        });
      })
    );
  return Dataset.of({ stream, size: sizeEffect });
}
