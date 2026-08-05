import { beforeEach, describe, expect, it } from "bun:test";

import { runSync } from "effect/Effect";

import { MessageRole, ScoreValue } from "../../harness/core";
import { makeEmptyBankingData, seedBankingCache } from "./environment";
import { bankingScorer } from "./scorer";
import { TerminationReason } from "./solver";
import type { Tau3Task } from "./types";
beforeEach(() => {
  const emptyDb = makeEmptyBankingData();
  seedBankingCache(emptyDb, []);
});

function makeTask(overrides?: Partial<Tau3Task>): Tau3Task {
  return {
    id: "task_test_001",
    user_scenario: { instructions: "test" },
    evaluation_criteria: {
      actions: [],
      communicate_info: [],
      reward_basis: ["DB"],
    },
    ...overrides,
  };
}
describe("bankingScorer", () => {
  it("returns Correct when reward >= 1", () => {
    const agentData = makeEmptyBankingData();
    const task = makeTask();
    const score = runSync(
      bankingScorer(
        {
          sample: {
            id: "samp_001",
            input: "test",
            target: { text: "" },
            metadata: {
              task,
              agentData,
              terminationReason: TerminationReason.UserStop,
            },
          },
          messages: [
            {
              role: MessageRole.System,
              content: "system prompt",
            },
            {
              role: MessageRole.Assistant,
              content: "Hello",
            },
          ],
          completed: true,
        },
        { text: "" }
      )
    );
    expect(score.value).toBe(ScoreValue.Correct);
  });
  it("returns Incorrect when reward < 1", () => {
    const agentData = makeEmptyBankingData();
    agentData.users.data["user_1"] = { name: "Test" };
    const task = makeTask({
      evaluation_criteria: {
        actions: [],
        communicate_info: [],
        reward_basis: ["DB"],
      },
    });
    const score = runSync(
      bankingScorer(
        {
          sample: {
            id: "samp_001",
            input: "test",
            target: { text: "" },
            metadata: {
              task,
              agentData,
              terminationReason: TerminationReason.UserStop,
            },
          },
          messages: [
            {
              role: MessageRole.System,
              content: "system prompt",
            },
          ],
          completed: true,
        },
        { text: "" }
      )
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
  });
  it("returns Incorrect when agentData is missing", () => {
    const task = makeTask();
    const score = runSync(
      bankingScorer(
        {
          sample: {
            id: "samp_001",
            input: "test",
            target: { text: "" },
            metadata: {
              task,
              terminationReason: TerminationReason.UserStop,
            },
          },
          messages: [],
          completed: true,
        },
        { text: "" }
      )
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toContain("Missing agentData");
  });
  it("includes breakdown in explanation", () => {
    const agentData = makeEmptyBankingData();
    const task = makeTask({
      evaluation_criteria: {
        actions: [],
        communicate_info: [],
        reward_basis: ["DB", "COMMUNICATE"],
      },
    });
    const score = runSync(
      bankingScorer(
        {
          sample: {
            id: "samp_001",
            input: "test",
            target: { text: "" },
            metadata: {
              task,
              agentData,
              terminationReason: TerminationReason.UserStop,
            },
          },
          messages: [
            {
              role: MessageRole.Assistant,
              content: "response",
            },
          ],
          completed: true,
        },
        { text: "" }
      )
    );
    expect(score.explanation).toContain("breakdown");
  });
  it("returns Incorrect on premature termination", () => {
    const agentData = makeEmptyBankingData();
    const task = makeTask();
    const score = runSync(
      bankingScorer(
        {
          sample: {
            id: "samp_001",
            input: "test",
            target: { text: "" },
            metadata: {
              task,
              agentData,
              terminationReason: TerminationReason.MaxSteps,
            },
          },
          messages: [],
          completed: true,
        },
        { text: "" }
      )
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toContain("prematurely");
  });
  it("collects tool calls from messages", () => {
    const agentData = makeEmptyBankingData();
    const task = makeTask({
      evaluation_criteria: {
        actions: [
          {
            action_id: "a1",
            name: "test_tool",
            arguments: {},
            requestor: "assistant",
          },
        ],
        communicate_info: [],
        reward_basis: ["ACTION"],
      },
    });
    const score = runSync(
      bankingScorer(
        {
          sample: {
            id: "samp_001",
            input: "test",
            target: { text: "" },
            metadata: {
              task,
              agentData,
              terminationReason: TerminationReason.UserStop,
            },
          },
          messages: [
            {
              role: MessageRole.Assistant,
              content: "response",
              toolCalls: [
                {
                  id: "call_001",
                  type: "function" as const,
                  function: {
                    name: "test_tool",
                    arguments: "{}",
                  },
                },
              ],
            },
          ],
          completed: true,
        },
        { text: "" }
      )
    );
    expect(score.value).toBe(ScoreValue.Correct);
  });
  it("includes user-simulator tool calls in ACTION scoring", () => {
    const agentData = makeEmptyBankingData();
    const task = makeTask({
      evaluation_criteria: {
        actions: [
          {
            action_id: "a1",
            name: "request_human_agent_transfer",
            arguments: {},
            requestor: "user",
          },
        ],
        communicate_info: [],
        reward_basis: ["ACTION"],
      },
    });
    const score = runSync(
      bankingScorer(
        {
          sample: {
            id: "samp_001",
            input: "test",
            target: { text: "" },
            metadata: {
              task,
              agentData,
              terminationReason: TerminationReason.UserStop,
              predictedUserToolCalls: [
                {
                  name: "request_human_agent_transfer",
                  arguments: {},
                  requestor: "user",
                },
              ],
            },
          },
          messages: [],
          completed: true,
        },
        { text: "" }
      )
    );
    expect(score.value).toBe(ScoreValue.Correct);
  });
});
