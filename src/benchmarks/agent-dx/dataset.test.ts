import { describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  agentDxTasksDir,
  listTaskIds,
  loadTask,
  readAgentDxMeta,
  taskToSample,
} from "./dataset";

describe("agent-dx dataset", () => {
  it("lists the checked-in tasks", () => {
    const ids = listTaskIds(agentDxTasksDir());
    expect(ids).toEqual([
      "agent-sdk-workflow",
      "basic-completion",
      "byok-config",
      "fallback-resilience",
      "image-input",
      "model-discovery",
      "preset-config",
      "provider-pinning",
      "streaming-usage",
      "structured-outputs",
      "tool-calling-loop",
      "web-search",
    ]);
  });

  it("lists the regression suite separately from the benchmark suite", () => {
    const regressionIds = listTaskIds(
      agentDxTasksDir(),
      undefined,
      "regression"
    );
    expect(regressionIds).toEqual(["compiled-esm-scaffold"]);

    const benchmarkIds = listTaskIds(agentDxTasksDir());
    expect(benchmarkIds).not.toContain("compiled-esm-scaffold");
  });

  it("allows an explicit subset to select cross-suite tasks", () => {
    const ids = listTaskIds(agentDxTasksDir(), [
      "compiled-esm-scaffold",
      "basic-completion",
    ]);
    expect(ids).toEqual(["compiled-esm-scaffold", "basic-completion"]);
  });

  it("selects nothing for an explicitly empty subset", () => {
    expect(listTaskIds(agentDxTasksDir(), [])).toEqual([]);
  });

  it("filters by task subset preserving order", () => {
    const ids = listTaskIds(agentDxTasksDir(), [
      "tool-calling-loop",
      "nonexistent",
    ]);
    expect(ids).toEqual(["tool-calling-loop"]);
  });

  it("drops a subset entry whose task.toml fails to load", () => {
    const dir = mkdtempSync(join(tmpdir(), "agent-dx-dataset-"));
    mkdirSync(join(dir, "broken-task"));
    writeFileSync(join(dir, "broken-task", "task.toml"), "not = [valid toml");
    expect(listTaskIds(dir, ["broken-task"])).toEqual([]);
  });

  it("loads and validates every task.toml", () => {
    for (const id of [
      ...listTaskIds(agentDxTasksDir()),
      ...listTaskIds(agentDxTasksDir(), undefined, "regression"),
    ]) {
      const task = loadTask(id, agentDxTasksDir());
      expect(task.id).toBe(id);
      expect(task.dockerImage).toBe("node:24-bookworm");
      expect(task.taskToml.agent.timeout_sec).toBeGreaterThan(0);
      expect(task.taskToml.verifier.timeout_sec).toBeGreaterThan(0);
    }
  });

  it("rejects task ids that are not a single safe path segment", () => {
    expect(() => loadTask("../outside", agentDxTasksDir())).toThrow(
      'task id "../outside" is not a valid task name'
    );
    expect(() => loadTask("a/b", agentDxTasksDir())).toThrow(
      "is not a valid task name"
    );
    expect(() => loadTask(".hidden", agentDxTasksDir())).toThrow(
      "is not a valid task name"
    );
  });

  it("round-trips sample metadata through readAgentDxMeta", () => {
    const task = loadTask("basic-completion", agentDxTasksDir());
    const sample = taskToSample(task);
    expect(sample.id).toBe("agent_dx-basic-completion");
    expect(sample.input).toContain("npm start");

    const meta = readAgentDxMeta(sample.metadata);
    expect(meta).toBeDefined();
    expect(meta?.taskId).toBe("basic-completion");
    expect(meta?.dockerImage).toBe("node:24-bookworm");
    expect(meta?.maxAgentTimeoutSec).toBe(900);
    expect(meta?.maxTestTimeoutSec).toBe(600);
    expect(meta?.taskEnv).toEqual({});
  });

  it("applies the agent timeout override", () => {
    const task = loadTask("basic-completion", agentDxTasksDir());
    const meta = readAgentDxMeta(taskToSample(task, 120).metadata);
    expect(meta?.maxAgentTimeoutSec).toBe(120);
  });

  it("returns undefined for non-agent-dx metadata", () => {
    expect(readAgentDxMeta(undefined)).toBeUndefined();
    expect(readAgentDxMeta({ taskId: "x" })).toBeUndefined();
  });
});
