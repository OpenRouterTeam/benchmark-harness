import { beforeEach, describe, expect, it } from "bun:test";

import { makeEmptyBankingData, seedBankingCache } from "./environment";
import { evaluateSimulation } from "./evaluator";
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
describe("evaluateSimulation", () => {
  describe("termination checks", () => {
    it("returns zero reward for MAX_STEPS termination", () => {
      const task = makeTask();
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.MaxSteps,
      });
      expect(result.reward).toBe(0);
      expect(result.note).toContain("prematurely");
    });
    it("returns zero reward for TOO_MANY_ERRORS termination", () => {
      const task = makeTask();
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.TooManyErrors,
      });
      expect(result.reward).toBe(0);
      expect(result.note).toContain("prematurely");
    });
    it("accepts AGENT_STOP termination", () => {
      const task = makeTask();
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.AgentStop,
      });
      expect(result.reward).toBe(1);
      expect(result.dbMatch).toBe(true);
    });
    it("accepts USER_STOP termination", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [],
          communicate_info: [],
          reward_basis: ["DB"],
        },
      });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(1);
      expect(result.dbMatch).toBe(true);
    });
  });
  describe("null criteria", () => {
    it("returns perfect reward when criteria is null", () => {
      const task = makeTask({ evaluation_criteria: null });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(1);
      expect(result.note).toContain("No evaluation criteria");
    });
    it("returns perfect reward when task is undefined", () => {
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task: undefined,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(1);
      expect(result.note).toContain("No evaluation criteria");
    });
  });
  describe("DB basis", () => {
    it("returns 1 when golden actions is null", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: null,
          communicate_info: [],
          reward_basis: ["DB"],
        },
      });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(1);
      expect(result.dbMatch).toBe(true);
    });
    it("returns 1 when agent DB matches empty golden DB", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [],
          communicate_info: [],
          reward_basis: ["DB"],
        },
      });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(1);
      expect(result.dbMatch).toBe(true);
    });
    it("returns 0 when DB hashes do not match", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [],
          communicate_info: [],
          reward_basis: ["DB"],
        },
      });
      const agentData = makeEmptyBankingData();
      agentData.users.data["user_1"] = { name: "Test User" };
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(0);
      expect(result.dbMatch).toBe(false);
    });
  });
  describe("ACTION basis", () => {
    it("returns 1 when no golden actions required", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [],
          communicate_info: [],
          reward_basis: ["ACTION"],
        },
      });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(1);
      expect(result.actionMet).toBe(true);
    });
    it("returns 1 when all golden actions are matched", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [
            {
              action_id: "a1",
              name: "get_user_information_by_id",
              arguments: { user_id: "user_123" },
              requestor: "assistant",
            },
          ],
          communicate_info: [],
          reward_basis: ["ACTION"],
        },
      });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [
          {
            name: "get_user_information_by_id",
            arguments: { user_id: "user_123" },
            requestor: "assistant",
          },
        ],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(1);
      expect(result.actionMet).toBe(true);
    });
    it("returns 0 when a golden action is missing", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [
            {
              action_id: "a1",
              name: "get_user_information_by_id",
              arguments: { user_id: "user_123" },
              requestor: "assistant",
            },
          ],
          communicate_info: [],
          reward_basis: ["ACTION"],
        },
      });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(0);
      expect(result.actionMet).toBe(false);
    });
  });
  describe("COMMUNICATE basis", () => {
    it("returns 1 when no communicate_info required", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [],
          communicate_info: [],
          reward_basis: ["COMMUNICATE"],
        },
      });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: ["Hello"],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(1);
      expect(result.communicateMet).toBe(true);
    });
    it("returns 1 when all communicate_info appears in texts", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [],
          communicate_info: ["hello", "world"],
          reward_basis: ["COMMUNICATE"],
        },
      });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: ["hello there", "world is great"],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(1);
      expect(result.communicateMet).toBe(true);
    });
    it("returns 0 when communicate_info is missing", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [],
          communicate_info: ["hello", "world"],
          reward_basis: ["COMMUNICATE"],
        },
      });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: ["hello there"],
        toolCalls: [],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(0);
      expect(result.communicateMet).toBe(false);
    });
  });
  describe("reward basis composition", () => {
    it("multiplies DB and ACTION when both in basis", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [
            {
              action_id: "a1",
              name: "some_tool",
              arguments: {},
              requestor: "assistant",
            },
          ],
          communicate_info: [],
          reward_basis: ["DB", "ACTION"],
        },
      });
      const agentData = makeEmptyBankingData();
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [
          {
            name: "some_tool",
            arguments: {},
            requestor: "assistant",
          },
        ],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(1);
      expect(result.breakdown.db).toBe(1);
      expect(result.breakdown.action).toBe(1);
    });
    it("returns 0 when DB basis fails even if ACTION passes", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [
            {
              action_id: "a1",
              name: "some_tool",
              arguments: {},
              requestor: "assistant",
            },
          ],
          communicate_info: [],
          reward_basis: ["DB", "ACTION"],
        },
      });
      const agentData = makeEmptyBankingData();
      agentData.users.data["user_1"] = { name: "Test" };
      const result = evaluateSimulation({
        task,
        agentData,
        assistantTexts: [],
        toolCalls: [
          {
            name: "some_tool",
            arguments: {},
            requestor: "assistant",
          },
        ],
        terminationReason: TerminationReason.UserStop,
      });
      expect(result.reward).toBe(0);
    });
  });
  describe("unsupported reward bases", () => {
    it("throws instead of silently satisfying an unimplemented reward basis", () => {
      const task = makeTask({
        evaluation_criteria: {
          actions: [],
          communicate_info: [],
          reward_basis: ["ENV_ASSERTION"],
        },
      });
      const agentData = makeEmptyBankingData();
      expect(() =>
        evaluateSimulation({
          task,
          agentData,
          assistantTexts: [],
          toolCalls: [],
          terminationReason: TerminationReason.UserStop,
        })
      ).toThrow(
        "Banking task task_test_001 uses unsupported reward basis: ENV_ASSERTION"
      );
    });
  });
});
