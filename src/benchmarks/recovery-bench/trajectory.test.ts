import { describe, expect, it } from "bun:test";

import { MessageRole } from "../../harness/core";
import { Either } from "../../internal/either";
import { assertLeft, assertRight } from "../../internal/testing";
import { parseTrajectory, planReplay } from "./trajectory";

function atif(steps: readonly unknown[], modelName = "claude-haiku"): string {
  return JSON.stringify({
    schema_version: "ATIF-v1.6",
    session_id: "s",
    agent: { name: "terminus-2", model_name: modelName },
    steps,
  });
}

function agentStep(
  message: string,
  keystrokes: readonly (readonly [string, number])[]
): unknown {
  return {
    step_id: 1,
    source: "agent",
    message,
    tool_calls: keystrokes.map(([k, duration]) => ({
      tool_call_id: "c",
      function_name: "terminus_2_command",
      arguments: { keystrokes: k, duration },
    })),
  };
}

describe("recovery-bench trajectory parsing", () => {
  it("extracts messages and executable commands from ATIF v1.6", () => {
    const parsed = parseTrajectory(
      atif([
        { step_id: 0, source: "user", message: "do the task" },
        agentStep("running", [
          ["ls -la\n", 0.5],
          ["cat foo.txt\n", 1],
        ]),
      ])
    );
    assertRight(parsed);
    expect(parsed.right.schemaVersion).toBe("ATIF-v1.6");
    expect(parsed.right.modelName).toBe("claude-haiku");
    expect(parsed.right.messages).toEqual([
      { role: MessageRole.User, content: "do the task" },
      { role: MessageRole.Assistant, content: "running" },
    ]);
    expect(parsed.right.commands.map((c) => c.command)).toEqual([
      "ls -la",
      "cat foo.txt",
    ]);
  });

  it("derives per-command timeouts from the recorded duration", () => {
    const parsed = parseTrajectory(
      atif([
        agentStep("", [
          ["sleep 1\n", 0.5],
          ["make\n", 30],
        ]),
      ])
    );
    assertRight(parsed);
    expect(parsed.right.commands.map((c) => c.timeoutSec)).toEqual([10, 305]);
  });

  it("treats control-only keystrokes as non-executable", () => {
    const parsed = parseTrajectory(
      atif([
        agentStep("", [
          ["C-c", 0.1],
          ["\n", 0.1],
          ["echo hi\n", 0.1],
        ]),
      ])
    );
    assertRight(parsed);
    expect(parsed.right.commands.map((c) => c.command)).toEqual([
      "",
      "",
      "echo hi",
    ]);
    const plan = planReplay(parsed.right.commands);
    expect(plan.commands).toEqual(["echo hi"]);
    expect(plan.totalExecutable).toBe(1);
    expect(plan.skippedInterrupted).toBe(0);
  });

  it("skips commands the original agent interrupted with a control sequence", () => {
    const parsed = parseTrajectory(
      atif([
        agentStep("", [
          ["python serve.py\n", 5],
          ["C-c", 0.1],
          ["ls\n", 0.5],
        ]),
      ])
    );
    assertRight(parsed);
    const plan = planReplay(parsed.right.commands);
    expect(plan.commands).toEqual(["ls"]);
    expect(plan.totalExecutable).toBe(2);
    expect(plan.skippedInterrupted).toBe(1);
  });

  it("ignores tool calls without keystrokes and defaults a missing message", () => {
    const parsed = parseTrajectory(
      atif([
        {
          step_id: 1,
          source: "agent",
          tool_calls: [{ tool_call_id: "c", function_name: "f" }],
        },
      ])
    );
    assertRight(parsed);
    expect(parsed.right.commands).toEqual([]);
    expect(parsed.right.messages).toEqual([
      { role: MessageRole.Assistant, content: "" },
    ]);
  });

  it("rejects invalid JSON and schema violations with a tagged error", () => {
    const notJson = parseTrajectory("{nope");
    assertLeft(notJson);
    expect(notJson.left._tag).toBe("TrajectoryParseError");
    expect(notJson.left.message).toContain("not valid JSON");

    const badSchema = parseTrajectory(
      JSON.stringify({
        schema_version: "ATIF-v1.6",
        steps: [{ source: "robot" }],
      })
    );
    expect(Either.isLeft(badSchema)).toBe(true);
    assertLeft(badSchema);
    expect(badSchema.left.message).toContain("ATIF validation");
  });
});
