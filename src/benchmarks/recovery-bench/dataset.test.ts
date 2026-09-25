import { afterAll, describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { runPromise } from "effect/Effect";
import { runCollect } from "effect/Stream";

import { assertLeft, assertRight } from "../../internal/testing";
import {
  makeRecoveryBenchDataset,
  readRecoveryBenchMeta,
  recoveryBenchSampleId,
  selectTrials,
} from "./dataset";
import type { RecoveryBenchManifest } from "./manifest";
import {
  buildRecoveryBenchManifest,
  RECOVERY_BENCH_MANIFEST,
  trajectoryUrl,
} from "./manifest";
import { MAX_RECOVERY_INSTRUCTION_BYTES, parseMemoryMb } from "./schema";
import {
  RECOVERY_BENCH_INITIAL_RUN,
  RECOVERY_BENCH_SOURCE_COMMIT,
  resetCheckoutCache,
  seedTasksRoot,
  TERMINAL_BENCH_2_SOURCE_COMMIT,
} from "./tasks-source";

const SHA = "a".repeat(64);

const SMALL_TRIAL = {
  taskId: "fix-build",
  trialDir: "01234567-fix-build__abc",
  trajectorySha256: SHA,
  trajectoryBytes: 100,
  fullContextBytes: 2000,
  replayCommands: 3,
} as const;

const HUGE_TRIAL = {
  ...SMALL_TRIAL,
  taskId: "huge-task",
  trialDir: "89abcdef-huge-task__xyz",
  fullContextBytes: MAX_RECOVERY_INSTRUCTION_BYTES + 1,
} as const;

const MANIFEST: RecoveryBenchManifest = {
  sourceCommit: RECOVERY_BENCH_SOURCE_COMMIT,
  run: RECOVERY_BENCH_INITIAL_RUN,
  trials: [SMALL_TRIAL, HUGE_TRIAL],
};

function makeFakeTasksDir(): string {
  const dir = mkdtempSync(join(tmpdir(), "recovery-bench-dataset-test-"));
  for (const taskId of ["fix-build", "huge-task"]) {
    const taskDir = join(dir, taskId);
    mkdirSync(join(taskDir, "tests"), { recursive: true });
    writeFileSync(
      join(taskDir, "task.toml"),
      [
        'version = "1.0"',
        "[metadata]",
        'difficulty = "medium"',
        'category = "software-engineering"',
        "[verifier]",
        "timeout_sec = 600.0",
        "[agent]",
        "timeout_sec = 900.0",
        "[environment]",
        `docker_image = "example/${taskId}:20251031"`,
        "cpus = 2",
        'memory = "4G"',
        "allow_internet = false",
      ].join("\n")
    );
    writeFileSync(join(taskDir, "instruction.md"), `Do ${taskId}.\n`);
  }
  return dir;
}

const fakeTasksDir = makeFakeTasksDir();
seedTasksRoot(fakeTasksDir);
afterAll(() => {
  resetCheckoutCache();
  rmSync(fakeTasksDir, { recursive: true, force: true });
});

describe("recovery-bench manifest", () => {
  it("committed manifest is pinned to the bundled Haiku run and has unique trials", () => {
    expect(RECOVERY_BENCH_MANIFEST.sourceCommit).toBe(
      RECOVERY_BENCH_SOURCE_COMMIT
    );
    expect(RECOVERY_BENCH_MANIFEST.run).toBe(RECOVERY_BENCH_INITIAL_RUN);
    const dirs = new Set(RECOVERY_BENCH_MANIFEST.trials.map((t) => t.trialDir));
    expect(dirs.size).toBe(RECOVERY_BENCH_MANIFEST.trials.length);
    expect(RECOVERY_BENCH_MANIFEST.trials.length).toBeGreaterThan(50);
  });

  it("rejects duplicate trial directories and malformed entries", () => {
    expect(() =>
      buildRecoveryBenchManifest({
        ...MANIFEST,
        trials: [SMALL_TRIAL, SMALL_TRIAL],
      })
    ).toThrow(/duplicate trialDir/);
    expect(() =>
      buildRecoveryBenchManifest({
        ...MANIFEST,
        trials: [{ ...SMALL_TRIAL, trajectorySha256: "short" }],
      })
    ).toThrow(/invalid/);
  });

  it("builds raw LFS media URLs from the pinned commit", () => {
    expect(trajectoryUrl(MANIFEST, SMALL_TRIAL)).toBe(
      `https://media.githubusercontent.com/media/letta-ai/recovery-bench/${RECOVERY_BENCH_SOURCE_COMMIT}/runs/${RECOVERY_BENCH_INITIAL_RUN}/${SMALL_TRIAL.trialDir}/agent/trajectory.json`
    );
  });
});

describe("recovery-bench trial selection", () => {
  it("drops trials whose full-context instruction exceeds the byte limit", () => {
    const selected = selectTrials(MANIFEST, undefined);
    assertRight(selected);
    expect(selected.right.map((t) => t.taskId)).toEqual(["fix-build"]);
    const raised = selectTrials(
      MANIFEST,
      undefined,
      MAX_RECOVERY_INSTRUCTION_BYTES + 1
    );
    assertRight(raised);
    expect(raised.right.length).toBe(2);
  });

  it("accepts task ids or trial dirs in the subset and rejects unknown ones", () => {
    const byDir = selectTrials(MANIFEST, [SMALL_TRIAL.trialDir]);
    assertRight(byDir);
    expect(byDir.right.length).toBe(1);
    const unknown = selectTrials(MANIFEST, ["fix-build", "nope"]);
    assertLeft(unknown);
    expect(unknown.left.message).toContain("nope");
  });

  it("parses Terminal-Bench 2.0 memory strings into MiB", () => {
    expect(parseMemoryMb("2G")).toBe(2048);
    expect(parseMemoryMb("512M")).toBe(512);
    expect(parseMemoryMb("lots")).toBeUndefined();
  });
});

describe("recovery-bench dataset", () => {
  it("streams samples with stable ids and validated metadata from a seeded checkout", async () => {
    const dataset = makeRecoveryBenchDataset({ manifest: MANIFEST });
    expect(await runPromise(dataset.size)).toBe(1);
    const samples = [...(await runPromise(runCollect(dataset.stream())))];
    expect(samples.length).toBe(1);
    const sample = samples[0];
    expect(sample?.id).toBe(recoveryBenchSampleId(SMALL_TRIAL));
    expect(sample?.id).toBe("recovery_bench-01234567-fix-build__abc");
    expect(sample?.input).toBe("Do fix-build.\n");
    expect(sample?.target).toEqual({ text: "fix-build" });
    const meta = readRecoveryBenchMeta(sample?.metadata);
    expect(meta).toEqual({
      taskId: "fix-build",
      trialDir: SMALL_TRIAL.trialDir,
      trajectoryUrl: trajectoryUrl(MANIFEST, SMALL_TRIAL),
      trajectorySha256: SHA,
      trajectoryBytes: 100,
      recoveryBenchCommit: RECOVERY_BENCH_SOURCE_COMMIT,
      terminalBenchCommit: TERMINAL_BENCH_2_SOURCE_COMMIT,
      dockerImage: "example/fix-build:20251031",
      maxAgentTimeoutSec: 900,
      maxTestTimeoutSec: 600,
      difficulty: "medium",
      category: "software-engineering",
      cpus: 2,
      memoryMb: 4096,
      gpus: 0,
      allowInternet: false,
    });
  });

  it("applies the agent timeout override and honours start/end windows", async () => {
    const dataset = makeRecoveryBenchDataset({
      manifest: MANIFEST,
      maxInstructionBytes: MAX_RECOVERY_INSTRUCTION_BYTES + 1,
      maxAgentTimeoutSec: 120,
      pageSize: 1,
    });
    const all = [...(await runPromise(runCollect(dataset.stream())))];
    expect(all.map((s) => readRecoveryBenchMeta(s.metadata)?.taskId)).toEqual([
      "fix-build",
      "huge-task",
    ]);
    expect(readRecoveryBenchMeta(all[0]?.metadata)?.maxAgentTimeoutSec).toBe(
      120
    );
    const second = [
      ...(await runPromise(runCollect(dataset.stream({ start: 1, end: 2 })))),
    ];
    expect(second.map((s) => s.target.text)).toEqual(["huge-task"]);
  });

  it("returns undefined metadata for samples from other benchmarks", () => {
    expect(readRecoveryBenchMeta({ taskId: "x" })).toBeUndefined();
    expect(readRecoveryBenchMeta("nope")).toBeUndefined();
  });
});
