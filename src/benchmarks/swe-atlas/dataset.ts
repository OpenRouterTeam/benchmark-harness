import { readFileSync, readdirSync, statSync } from "node:fs";
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
import { parseSchema } from "../../internal/zod";
import type { SweAtlasTask, SweAtlasTrack } from "./schema";
import { SWE_ATLAS_TRACKS, SweAtlasTaskTomlSchema } from "./schema";
import { ensureTasksCheckedOutEffect, trackDir } from "./tasks-source";

export const SWE_ATLAS_DATASET_IDS = {
  qa: "swe_atlas_qa",
  tw: "swe_atlas_tw",
  rf: "swe_atlas_rf",
} as const satisfies Record<SweAtlasTrack, string>;

function isTrack(value: unknown): value is SweAtlasTrack {
  return (
    typeof value === "string" &&
    SWE_ATLAS_TRACKS.some((track) => track === value)
  );
}

const DEFAULT_CPUS = 16;

const DEFAULT_MEMORY_MB = 16384;

const DEFAULT_ALLOW_INTERNET = true;

export function loadTask(
  taskId: string,
  track: SweAtlasTrack,
  tasksRoot: string
): SweAtlasTask {
  const taskDir = join(trackDir(tasksRoot, track), taskId);
  const tomlPath = join(taskDir, "task.toml");
  const raw = readFileSync(tomlPath, "utf8");
  const tomlObj = Either.try(() => tomlParse(raw));
  if (Either.isLeft(tomlObj)) {
    throw new Error(
      `swe-atlas task "${taskId}" task.toml failed to parse: ${String(tomlObj.left)}`
    );
  }
  const parsed = parseSchema(SweAtlasTaskTomlSchema, tomlObj.right);
  if (Either.isLeft(parsed)) {
    throw new Error(
      `swe-atlas task "${taskId}" task.toml failed validation: ${parsed.left.message}`
    );
  }
  return {
    id: taskId,
    track,
    taskToml: parsed.right,
    taskDir,
    testDir: join(taskDir, "tests"),
    instructionPath: join(taskDir, "instruction.md"),
    dockerImage: parsed.right.environment.docker_image,
  };
}

export function listTaskIds(
  track: SweAtlasTrack,
  tasksRoot: string,
  taskSubset?: readonly string[]
): readonly string[] {
  const dir = trackDir(tasksRoot, track);
  const onDisk = readdirSync(dir).filter((entry) => {
    if (!entry.startsWith("task-")) {
      return false;
    }
    try {
      return statSync(join(dir, entry)).isDirectory();
    } catch {
      return false;
    }
  });
  if (taskSubset !== undefined && taskSubset.length > 0) {
    const diskSet = new Set(onDisk);
    return taskSubset.filter((id) => diskSet.has(id));
  }
  return [...onDisk].toSorted();
}

export interface SweAtlasSampleMeta {
  readonly taskId: string;
  readonly track: SweAtlasTrack;
  readonly dockerImage: string;
  readonly maxAgentTimeoutSec: number;
  readonly maxTestTimeoutSec: number;
  readonly cpus: number;
  readonly memoryMb: number;
  readonly allowInternet: boolean;
  readonly category: string;
  reward?: number;
  verifierOutput?: string;
}

export function taskToSample(
  task: SweAtlasTask,
  maxAgentTimeoutSecOverride?: number
): Sample {
  const instruction = readFileSync(task.instructionPath, "utf8");
  return {
    id: `${SWE_ATLAS_DATASET_IDS[task.track]}-${task.id}`,
    input: instruction,
    target: { text: task.id },
    metadata: {
      taskId: task.id,
      track: task.track,
      dockerImage: task.dockerImage,
      maxAgentTimeoutSec:
        maxAgentTimeoutSecOverride ?? task.taskToml.agent.timeout_sec,
      maxTestTimeoutSec: task.taskToml.verifier.timeout_sec,
      cpus: task.taskToml.environment.cpus,
      memoryMb: task.taskToml.environment.memory_mb,
      allowInternet: task.taskToml.environment.allow_internet,
      category: task.taskToml.metadata.category ?? task.track,
    },
  };
}

export function readSweAtlasMeta(
  metadata?: Readonly<Record<string, unknown>>
): SweAtlasSampleMeta | undefined {
  if (metadata === undefined) {
    return undefined;
  }
  const taskId = metadata["taskId"];
  const track = metadata["track"];
  const dockerImage = metadata["dockerImage"];
  const maxAgentTimeoutSec = metadata["maxAgentTimeoutSec"];
  const maxTestTimeoutSec = metadata["maxTestTimeoutSec"];
  if (
    typeof taskId !== "string" ||
    !isTrack(track) ||
    typeof dockerImage !== "string" ||
    typeof maxAgentTimeoutSec !== "number" ||
    typeof maxTestTimeoutSec !== "number"
  ) {
    return undefined;
  }
  const category = metadata["category"];
  const cpus = metadata["cpus"];
  const memoryMb = metadata["memoryMb"];
  const allowInternet = metadata["allowInternet"];
  const reward = metadata["reward"];
  const verifierOutput = metadata["verifierOutput"];
  return {
    taskId,
    track,
    dockerImage,
    maxAgentTimeoutSec,
    maxTestTimeoutSec,
    cpus: typeof cpus === "number" ? cpus : DEFAULT_CPUS,
    memoryMb: typeof memoryMb === "number" ? memoryMb : DEFAULT_MEMORY_MB,
    allowInternet:
      typeof allowInternet === "boolean"
        ? allowInternet
        : DEFAULT_ALLOW_INTERNET,
    category: typeof category === "string" ? category : track,
    ...(typeof reward === "number" && { reward }),
    ...(typeof verifierOutput === "string" && { verifierOutput }),
  };
}

export interface SweAtlasDatasetConfig {
  readonly track: SweAtlasTrack;
  readonly taskSubset?: readonly string[];
  readonly maxAgentTimeoutSec?: number;
  readonly pageSize?: number;
}

export function makeSweAtlasDatasetLayer(
  config: SweAtlasDatasetConfig
): Layer<Dataset> {
  const pageSize = config.pageSize ?? 20;
  const { track, taskSubset, maxAgentTimeoutSec } = config;
  const makeService = succeed(
    buildDatasetService({ track, pageSize, taskSubset, maxAgentTimeoutSec })
  );
  return effect(Dataset, makeService);
}

function buildDatasetService(opts: {
  readonly track: SweAtlasTrack;
  readonly pageSize: number;
  readonly taskSubset?: readonly string[];
  readonly maxAgentTimeoutSec?: number;
}): ReturnType<typeof Dataset.of> {
  const { track, pageSize, taskSubset, maxAgentTimeoutSec } = opts;
  const tasksRootEffect = ensureTasksCheckedOutEffect().pipe(
    mapError((e) => new DatasetError({ message: e.message }))
  );
  const sizeEffect: Effect<number, DatasetError> = tasksRootEffect.pipe(
    flatMap((root) => succeed(listTaskIds(track, root, taskSubset).length))
  );
  const stream = (opts2?: DatasetStreamOptions): Stream<Sample, DatasetError> =>
    fromEffect(tasksRootEffect).pipe(
      flatMapStream((root: string) => {
        const allIds = listTaskIds(track, root, taskSubset);
        const start = opts2?.start ?? 0;
        const requestedEnd = opts2?.end ?? allIds.length;
        const end = Math.min(requestedEnd, allIds.length);
        const pageIds = allIds.slice(start, end);
        return paginateChunkEffect(0, (offset: number) => {
          const pageSlice = pageIds.slice(offset, offset + pageSize);
          const built = Either.try(() =>
            pageSlice.map((id) =>
              taskToSample(loadTask(id, track, root), maxAgentTimeoutSec)
            )
          );
          if (Either.isLeft(built)) {
            return fail(
              new DatasetError({
                message: `Failed to load swe-atlas ${track} tasks at offset ${offset}: ${String(built.left)}`,
              })
            );
          }
          const samples = built.right;
          const nextOffset = offset + pageSize;
          const hasMore = nextOffset < pageIds.length;
          return succeed([
            fromIterable(samples),
            hasMore ? some(nextOffset) : none(),
          ] as const);
        });
      })
    );
  return Dataset.of({ stream, size: sizeEffect });
}
