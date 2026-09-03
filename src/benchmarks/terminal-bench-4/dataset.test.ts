import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { assertLeft, assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import {
  TERMINAL_BENCH_4_DATASET_ID,
  dockerfileUser,
  listComposeTaskIds,
  listTaskIds,
  loadTask,
  readTerminalBench4Meta,
  taskToSample,
  toModalGpu,
  toSandboxResources,
} from "./dataset";
import { TERMINAL_BENCH_4_IMAGES } from "./images";
import { TaskTomlSchema } from "./schema";
import {
  ensureTasksCheckedOut,
  resetCheckoutCache,
  tasksDir as tasksDirOf,
} from "./tasks-source";

const NETWORK = describe.skipIf(Boolean(process.env["CI"]));

const BASE_TOML = `
[task]
name = "fixture"

[metadata]
category = "software-engineering"

[agent]
timeout_sec = 3600

[verifier]
timeout_sec = 600
environment_mode = "separate"

[environment]
cpus = 2
memory_mb = 4096
storage_mb = 10240
gpus = 0
`;

const GPU_TOML = `
artifacts = ["/app/out", { source = "/data", exclude = ["*.tmp"] }]

[task]
name = "gpu-fixture"

[metadata]
category = "machine-learning"

[agent]
timeout_sec = 7200

[verifier]
timeout_sec = 900
environment_mode = "separate"
env = { TB_SEED = "1" }

[[verifier.collect]]
command = "cp /var/log/app.log /logs/artifacts/"

[verifier.environment]
cpus = 16
memory_mb = 32768
storage_mb = 1024000
gpus = 1
gpu_types = ["H100"]

[environment]
cpus = 8
memory_mb = 16384
storage_mb = 102400
gpus = 1
gpu_types = ["H100"]
allow_internet = false
env = { HF_HOME = "/cache" }
`;

function writeTask(
  root: string,
  id: string,
  toml: string,
  compose = false,
  dockerfile = "FROM ubuntu:24.04\nWORKDIR /app\n"
) {
  const dir = join(root, id);
  mkdirSync(join(dir, "environment"), { recursive: true });
  writeFileSync(join(dir, "environment", "Dockerfile"), dockerfile);
  writeFileSync(join(dir, "task.toml"), toml);
  writeFileSync(join(dir, "instruction.md"), `Solve ${id}.\n`);
  if (compose) {
    writeFileSync(
      join(dir, "environment", "docker-compose.yaml"),
      "services: {}\n"
    );
  }
}

let fixtureRoot = "";
beforeAll(() => {
  fixtureRoot = mkdtempSync(join(tmpdir(), "tb4-fixture-"));
  writeTask(fixtureRoot, "b-cpu", BASE_TOML);
  writeTask(fixtureRoot, "a-gpu", GPU_TOML);
  writeTask(fixtureRoot, "c-compose", BASE_TOML, true);
  writeTask(
    fixtureRoot,
    "d-nonroot",
    BASE_TOML,
    false,
    "FROM ubuntu:24.04\nUSER root\nRUN useradd agent\nuser agent\n"
  );
  mkdirSync(join(fixtureRoot, ".hidden"));
  writeFileSync(join(fixtureRoot, "README.md"), "not a task\n");
});
afterAll(() => {
  rmSync(fixtureRoot, { recursive: true, force: true });
});

describe("terminal-bench-4 task.toml schema", () => {
  it("rejects a manifest whose verifier is not in separate mode", () => {
    assertLeft(
      parseSchema(TaskTomlSchema, {
        task: { name: "x" },
        metadata: { category: "c" },
        agent: { timeout_sec: 10 },
        verifier: { timeout_sec: 10, environment_mode: "same" },
        environment: { cpus: 1, memory_mb: 1, storage_mb: 1 },
      })
    );
  });

  it("defaults gpus, gpu_types, allow_internet, env, artifacts and collect", () => {
    const result = parseSchema(TaskTomlSchema, {
      task: { name: "x" },
      metadata: { category: "c" },
      agent: { timeout_sec: 10 },
      verifier: { timeout_sec: 10, environment_mode: "separate" },
      environment: { cpus: 1, memory_mb: 1, storage_mb: 1 },
    });
    assertRight(result);
    expect(result.right.environment.gpus).toBe(0);
    expect(result.right.environment.gpu_types).toEqual([]);
    expect(result.right.environment.allow_internet).toBe(true);
    expect(result.right.environment.env).toEqual({});
    expect(result.right.artifacts).toEqual([]);
    expect(result.right.verifier.collect).toEqual([]);
    expect(result.right.verifier.env).toEqual({});
  });
});

describe("terminal-bench-4 task listing", () => {
  it("lists non-compose task directories sorted and skips hidden dirs and files", () => {
    expect(listTaskIds(fixtureRoot)).toEqual(["a-gpu", "b-cpu", "d-nonroot"]);
  });

  it("lists compose tasks separately", () => {
    expect(listComposeTaskIds(fixtureRoot)).toEqual(["c-compose"]);
  });

  it("honors a task subset in the requested order and drops unknown or compose ids", () => {
    expect(
      listTaskIds(fixtureRoot, ["b-cpu", "nope", "c-compose", "a-gpu"])
    ).toEqual(["b-cpu", "a-gpu"]);
  });
});

describe("terminal-bench-4 dockerfileUser", () => {
  it("returns undefined when there is no USER or the last USER is root", () => {
    expect(dockerfileUser("FROM x\nRUN true\n")).toBeUndefined();
    expect(dockerfileUser("FROM x\nUSER agent\nUSER root\n")).toBeUndefined();
    expect(dockerfileUser("FROM x\nUSER 0\n")).toBeUndefined();
  });

  it("returns the last USER directive, ignoring case and indentation", () => {
    expect(dockerfileUser("FROM x\nUSER root\n  user nobody\n")).toBe("nobody");
    expect(dockerfileUser("FROM x\nUSER agent:agent\n")).toBe("agent:agent");
  });
});

describe("terminal-bench-4 GPU mapping", () => {
  const env = {
    cpus: 1,
    memory_mb: 1,
    storage_mb: 1,
    allow_internet: true,
    env: {},
  };

  it("returns no gpu for cpu-only tasks", () => {
    expect(toModalGpu({ ...env, gpus: 0, gpu_types: [] })).toBeUndefined();
  });

  it("maps a single accelerator to its bare type", () => {
    expect(toModalGpu({ ...env, gpus: 1, gpu_types: ["H100"] })).toBe("H100");
  });

  it("maps multiple accelerators with a count suffix", () => {
    expect(toModalGpu({ ...env, gpus: 2, gpu_types: ["A100"] })).toBe("A100:2");
  });

  it("refuses to guess an accelerator when gpu_types is empty", () => {
    expect(() => toModalGpu({ ...env, gpus: 1, gpu_types: [] })).toThrow(
      /refusing to guess/
    );
  });

  it("omits the gpu key entirely for cpu-only sandbox resources", () => {
    const resources = toSandboxResources({ ...env, gpus: 0, gpu_types: [] });
    expect(Object.hasOwn(resources, "gpu")).toBe(false);
  });
});

describe("terminal-bench-4 taskToSample", () => {
  it("builds a stable id, instruction input and resource metadata for a cpu task", () => {
    const sample = taskToSample(loadTask("b-cpu", fixtureRoot));
    expect(sample.id).toBe(`${TERMINAL_BENCH_4_DATASET_ID}-b-cpu`);
    expect(sample.input).toBe("Solve b-cpu.\n");
    expect(sample.target).toEqual({ text: "b-cpu" });
    const meta = readTerminalBench4Meta(sample.metadata);
    expect(meta).toBeDefined();
    expect(meta?.maxAgentTimeoutSec).toBe(3600);
    expect(meta?.maxTestTimeoutSec).toBe(600);
    expect(meta?.agentEnv).toEqual({
      cpus: 2,
      memoryMb: 4096,
      env: {},
      allowInternet: true,
    });
    expect(meta?.verifierEnv).toEqual(meta?.agentEnv);
    expect(meta?.imageUser).toBeUndefined();
  });

  it("records the image's final non-root USER so the run can report the root deviation", () => {
    const meta = readTerminalBench4Meta(
      taskToSample(loadTask("d-nonroot", fixtureRoot)).metadata
    );
    expect(meta?.imageUser).toBe("agent");
  });

  it("propagates gpu, env vars, artifacts and collect hooks for a gpu task", () => {
    const meta = readTerminalBench4Meta(
      taskToSample(loadTask("a-gpu", fixtureRoot)).metadata
    );
    expect(meta?.agentEnv).toEqual({
      cpus: 8,
      memoryMb: 16384,
      gpu: "H100",
      env: { HF_HOME: "/cache" },
      allowInternet: false,
    });
    expect(meta?.verifierEnv).toEqual({
      cpus: 16,
      memoryMb: 32768,
      gpu: "H100",
      env: { TB_SEED: "1" },
      allowInternet: true,
    });
    expect(meta?.artifacts).toEqual([
      "/app/out",
      { source: "/data", exclude: ["*.tmp"] },
    ]);
    expect(meta?.collect).toEqual([
      { command: "cp /var/log/app.log /logs/artifacts/" },
    ]);
  });

  it("applies an agent timeout override without touching the verifier timeout", () => {
    const meta = readTerminalBench4Meta(
      taskToSample(loadTask("b-cpu", fixtureRoot), 60).metadata
    );
    expect(meta?.maxAgentTimeoutSec).toBe(60);
    expect(meta?.maxTestTimeoutSec).toBe(600);
  });

  it("refuses to build a sample for a compose task", () => {
    expect(() => taskToSample(loadTask("c-compose", fixtureRoot))).toThrow(
      /docker-compose/
    );
  });

  it("returns undefined for metadata that is not terminal-bench-4 metadata", () => {
    expect(readTerminalBench4Meta(undefined)).toBeUndefined();
    expect(readTerminalBench4Meta({ taskId: "x" })).toBeUndefined();
  });
});

NETWORK("terminal-bench-4 pinned checkout", () => {
  let tasksDir = "";
  let savedCacheDisable: string | undefined;
  beforeAll(async () => {
    resetCheckoutCache();
    savedCacheDisable = process.env.BENCH_DATASET_CACHE_DISABLE;
    process.env.BENCH_DATASET_CACHE_DISABLE ??= "0";
    tasksDir = tasksDirOf(await ensureTasksCheckedOut());
  }, 300_000);
  afterAll(() => {
    if (savedCacheDisable === undefined) {
      delete process.env.BENCH_DATASET_CACHE_DISABLE;
    } else {
      process.env.BENCH_DATASET_CACHE_DISABLE = savedCacheDisable;
    }
  });

  it("exposes 55 runnable tasks and 11 compose tasks", () => {
    expect(listTaskIds(tasksDir)).toHaveLength(55);
    expect(listComposeTaskIds(tasksDir)).toHaveLength(11);
  });

  it("parses every manifest and maps exactly three H100 tasks", () => {
    const gpuTasks = listTaskIds(tasksDir)
      .map((id) => taskToSample(loadTask(id, tasksDir)))
      .flatMap((sample) => {
        const meta = readTerminalBench4Meta(sample.metadata);
        return meta?.agentEnv.gpu === undefined ? [] : [meta.taskId];
      });
    expect(gpuTasks).toEqual([
      "fp8-rmsnorm-gemm",
      "jax-speedrun-gpu",
      "math-eval-grader",
    ]);
    for (const id of listComposeTaskIds(tasksDir)) {
      expect(loadTask(id, tasksDir).composeFile).toBeDefined();
    }
  });

  it("ships Modal images for exactly the runnable task set", () => {
    expect([...TERMINAL_BENCH_4_IMAGES.images.keys()].sort()).toEqual(
      listTaskIds(tasksDir)
    );
  });

  it("identifies exactly three tasks whose agent image runs as a non-root user", () => {
    const nonRoot = listTaskIds(tasksDir).flatMap((id) => {
      const user = loadTask(id, tasksDir).imageUser;
      return user === undefined ? [] : [`${id}:${user}`];
    });
    expect(nonRoot).toEqual([
      "fp8-rmsnorm-gemm:agent",
      "risk-scorer-replay:nobody",
      "rs-archive-clone:agent",
    ]);
  });
});
