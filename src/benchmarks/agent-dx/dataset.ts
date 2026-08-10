import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

import { fromIterable } from "effect/Chunk";
import type { Effect } from "effect/Effect";
import { fail, map, succeed, suspend } from "effect/Effect";
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
import { wLog } from "../../internal/log";
import { parseSchema } from "../../internal/zod";
import type { AgentDxSuite, AgentDxTask } from "./schema";
import { AgentDxTaskTomlSchema, DEFAULT_AGENT_DX_SUITE } from "./schema";

export const AGENT_DX_DATASET_ID = "agent_dx" as const;

export function agentDxTasksDir(): string {
  const override = process.env["AGENT_DX_TASKS_DIR"];
  if (override !== undefined && override.length > 0) {
    return override;
  }
  const here = import.meta.dirname;
  const inPackage = join(here, "tasks");
  if (existsSync(inPackage)) {
    return inPackage;
  }
  return join(here, "agent-dx-tasks");
}

const SAFE_TASK_ID = /^[a-z0-9][a-z0-9_-]*$/;

export function loadTask(taskId: string, tasksDir: string): AgentDxTask {
  if (!SAFE_TASK_ID.test(taskId)) {
    throw new Error(`agent-dx task id "${taskId}" is not a valid task name`);
  }
  const taskDir = join(tasksDir, taskId);
  const raw = readFileSync(join(taskDir, "task.toml"), "utf8");
  const tomlObj = Either.try(() => tomlParse(raw));
  if (Either.isLeft(tomlObj)) {
    throw new Error(
      `agent-dx task "${taskId}" task.toml failed to parse: ${String(tomlObj.left)}`
    );
  }
  const parsed = parseSchema(AgentDxTaskTomlSchema, tomlObj.right);
  if (Either.isLeft(parsed)) {
    throw new Error(
      `agent-dx task "${taskId}" task.toml failed validation: ${parsed.left.message}`
    );
  }
  return {
    id: taskId,
    taskToml: parsed.right,
    taskDir,
    testDir: join(taskDir, "tests"),
    testScript: join(taskDir, "tests", "test.sh"),
    instructionPath: join(taskDir, "instruction.md"),
    dockerImage: parsed.right.environment.docker_image,
  };
}

export function listTaskIds(
  tasksDir: string,
  taskSubset?: readonly string[],
  suite: AgentDxSuite = DEFAULT_AGENT_DX_SUITE
): readonly string[] {
  const onDisk = readdirSync(tasksDir).filter((entry) => {
    if (entry.startsWith(".")) {
      return false;
    }
    try {
      return statSync(join(tasksDir, entry)).isDirectory();
    } catch {
      return false;
    }
  });

  if (taskSubset !== undefined) {
    const diskSet = new Set(onDisk);
    const missing = taskSubset.filter((id) => !diskSet.has(id));
    if (missing.length > 0) {
      wLog(
        "agent-dx: task subset names tasks with no directory on disk; they will not run",
        {
          missing_task_ids: missing,
        }
      );
    }
    return taskSubset.filter((id) => {
      if (!diskSet.has(id)) {
        return false;
      }
      const loaded = Either.try(() => loadTask(id, tasksDir));
      if (Either.isLeft(loaded)) {
        wLog("agent-dx: task subset entry failed to load; it will not run", {
          task_id: id,
          error: String(loaded.left),
        });
        return false;
      }
      return true;
    });
  }
  return [...onDisk].sort().filter((id) => {
    const { task } = loadTask(id, tasksDir).taskToml;
    return !task.disabled && task.suite === suite;
  });
}

export interface AgentDxSampleMeta {
  readonly taskId: string;
  readonly dockerImage: string;
  readonly maxAgentTimeoutSec: number;
  readonly maxTestTimeoutSec: number;
  readonly difficulty: string;
  readonly category: string;
  readonly alignmentCriteria: readonly string[];
  readonly taskEnv: Readonly<Record<string, string>>;
  reward?: number;
  testOutput?: string;
}

export function taskToSample(
  task: AgentDxTask,
  maxAgentTimeoutSecOverride?: number
): Sample {
  const instruction = readFileSync(task.instructionPath, "utf8");
  return {
    id: `${AGENT_DX_DATASET_ID}-${task.id}`,
    input: instruction,
    target: { text: task.id },
    metadata: {
      taskId: task.id,
      dockerImage: task.dockerImage,
      maxAgentTimeoutSec:
        maxAgentTimeoutSecOverride ?? task.taskToml.agent.timeout_sec,
      maxTestTimeoutSec: task.taskToml.verifier.timeout_sec,
      difficulty: task.taskToml.metadata.difficulty,
      category: task.taskToml.metadata.category,
      alignmentCriteria: task.taskToml.metadata.alignment_criteria,
      taskEnv: task.taskToml.environment.env,
    },
  };
}

export function readAgentDxMeta(
  metadata: Readonly<Record<string, unknown>> | undefined
): AgentDxSampleMeta | undefined {
  if (metadata === undefined) {
    return undefined;
  }
  const taskId = metadata["taskId"];
  const dockerImage = metadata["dockerImage"];
  const maxAgentTimeoutSec = metadata["maxAgentTimeoutSec"];
  const maxTestTimeoutSec = metadata["maxTestTimeoutSec"];
  const difficulty = metadata["difficulty"];
  const category = metadata["category"];
  if (
    typeof taskId !== "string" ||
    typeof dockerImage !== "string" ||
    typeof maxAgentTimeoutSec !== "number" ||
    typeof maxTestTimeoutSec !== "number" ||
    typeof difficulty !== "string" ||
    typeof category !== "string"
  ) {
    return undefined;
  }
  const reward = metadata["reward"];
  const testOutput = metadata["testOutput"];
  return {
    taskId,
    dockerImage,
    maxAgentTimeoutSec,
    maxTestTimeoutSec,
    difficulty,
    category,
    alignmentCriteria: readStringArray(metadata["alignmentCriteria"]),
    taskEnv: readTaskEnv(metadata["taskEnv"]),
    ...(typeof reward === "number" && { reward }),
    ...(typeof testOutput === "string" && { testOutput }),
  };
}

function readStringArray(value: unknown): readonly string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.filter((entry): entry is string => typeof entry === "string");
}

function readTaskEnv(value: unknown): Readonly<Record<string, string>> {
  if (typeof value !== "object" || value === null) {
    return {};
  }
  const entries = Object.entries(value).flatMap(
    ([key, entryValue]): readonly [string, string][] =>
      typeof entryValue === "string" ? [[key, entryValue]] : []
  );
  return Object.fromEntries(entries);
}

export interface AgentDxDatasetConfig {
  readonly taskSubset?: readonly string[];
  readonly suite?: AgentDxSuite;
  readonly maxAgentTimeoutSec?: number;
  readonly pageSize?: number;
}

export function makeAgentDxDatasetLayer(
  config?: AgentDxDatasetConfig
): Layer<Dataset> {
  const pageSize = config?.pageSize ?? 20;
  return effect(
    Dataset,
    succeed(
      buildDatasetService({
        pageSize,
        ...(config?.taskSubset !== undefined && {
          taskSubset: config.taskSubset,
        }),
        ...(config?.suite !== undefined && { suite: config.suite }),
        ...(config?.maxAgentTimeoutSec !== undefined && {
          maxAgentTimeoutSec: config.maxAgentTimeoutSec,
        }),
      })
    )
  );
}

function buildDatasetService(opts: {
  readonly pageSize: number;
  readonly taskSubset?: readonly string[];
  readonly suite?: AgentDxSuite;
  readonly maxAgentTimeoutSec?: number;
}): ReturnType<typeof Dataset.of> {
  const { pageSize, taskSubset, suite, maxAgentTimeoutSec } = opts;
  const tasksDir = agentDxTasksDir();

  const listEffect: Effect<readonly string[], DatasetError> = suspend(() => {
    const listed = Either.try(() => listTaskIds(tasksDir, taskSubset, suite));
    return Either.isLeft(listed)
      ? fail(
          new DatasetError({
            message: `Failed to list agent-dx tasks in ${tasksDir}: ${String(listed.left)}`,
          })
        )
      : succeed(listed.right);
  });

  const sizeEffect: Effect<number, DatasetError> = map(
    listEffect,
    (ids) => ids.length
  );

  const stream = (opts2?: DatasetStreamOptions): Stream<Sample, DatasetError> =>
    flatMapStream(fromEffect(listEffect), (allIds) => {
      const start = opts2?.start ?? 0;
      const requestedEnd = opts2?.end ?? allIds.length;
      const end = Math.min(requestedEnd, allIds.length);
      const pageIds = allIds.slice(start, end);

      return paginateChunkEffect(0, (offset: number) => {
        const pageSlice = pageIds.slice(offset, offset + pageSize);
        const built = Either.try(() =>
          pageSlice.map((id) =>
            taskToSample(loadTask(id, tasksDir), maxAgentTimeoutSec)
          )
        );
        if (Either.isLeft(built)) {
          return fail(
            new DatasetError({
              message: `Failed to load agent-dx tasks at offset ${offset}: ${String(built.left)}`,
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
    });

  return Dataset.of({ stream, size: sizeEffect });
}
