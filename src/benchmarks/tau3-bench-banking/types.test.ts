import { describe, expect, it } from "bun:test";

import { assertRight, assertLeft } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { BANKING_TABLES, Tau3TaskSchema, RewardType } from "./types";
describe("tau3-bench-banking types", () => {
  describe("Tau3TaskSchema", () => {
    it("parses task_001 (basic task without initial_state)", () => {
      const task = {
        id: "task_001",
        description: {
          purpose: "Task: task_001",
          relevant_policies: null,
          notes: null,
        },
        user_scenario: {
          persona: null,
          instructions: "You are playing the role of a customer...",
        },
        initial_state: null,
        evaluation_criteria: {
          actions: [
            {
              name: "apply_for_credit_card",
              arguments: {
                card_type: "Gold Rewards Card",
                customer_name: "Sarah Bosch",
              },
              requestor: "user",
              action_id: "001_0",
            },
          ],
          communicate_info: [],
          reward_basis: ["DB"],
        },
        annotations: null,
        user_tools: ["apply_for_credit_card"],
        required_documents: ["doc_credit_cards_gold_rewards_card_001"],
      };
      const result = parseSchema(Tau3TaskSchema, task);
      assertRight(result);
      expect(result.right.id).toBe("task_001");
      expect(result.right.user_scenario.instructions).toBe(
        "You are playing the role of a customer..."
      );
      expect(result.right.evaluation_criteria?.reward_basis).toEqual(["DB"]);
    });
    it("parses task with non-null initial_state", () => {
      const task = {
        id: "task_026",
        description: {
          purpose: "Task: task_026",
          relevant_policies: null,
          notes: null,
        },
        user_scenario: {
          persona: null,
          instructions:
            "You have noticed that your recent cash back rewards dont add up correctly...",
        },
        initial_state: {
          initialization_data: {
            agent_data: {
              task_config: {
                data: { dispute_settings: { auto_resolve_disputes: true } },
              },
            },
          },
          initialization_actions: null,
          message_history: null,
        },
        evaluation_criteria: {
          actions: [
            {
              name: "log_verification",
              arguments: {},
              requestor: "user",
              action_id: "026_0",
            },
          ],
          communicate_info: [],
          reward_basis: ["DB"],
        },
        annotations: null,
        user_tools: [],
        required_documents: [],
      };
      const result = parseSchema(Tau3TaskSchema, task);
      assertRight(result);
      expect(result.right.id).toBe("task_026");
      expect(
        result.right.initial_state?.initialization_data?.agent_data
      ).toEqual({
        task_config: {
          data: { dispute_settings: { auto_resolve_disputes: true } },
        },
      });
    });
    it("tolerates unknown keys in user_scenario (persoca typo key)", () => {
      const task = {
        id: "task_091",
        description: {
          purpose: "Task: task_091",
          relevant_policies: null,
          notes: null,
        },
        user_scenario: {
          persoca: null,
          persona: null,
          instructions:
            "You are Tyler Washington, a 24-year-old software engineer...",
        },
        initial_state: null,
        evaluation_criteria: {
          actions: [
            {
              name: "test_action",
              arguments: {},
              requestor: "user",
              action_id: "091_0",
            },
          ],
          communicate_info: [],
          reward_basis: ["DB"],
        },
        annotations: null,
        user_tools: [],
        required_documents: [],
      };
      const result = parseSchema(Tau3TaskSchema, task);
      assertRight(result);
      expect(result.right.user_scenario.instructions).toBe(
        "You are Tyler Washington, a 24-year-old software engineer..."
      );
      expect(result.right.user_scenario.persoca).toBeNull();
    });
    it("validates action structure within evaluation_criteria", () => {
      const task = {
        id: "task_001",
        description: null,
        user_scenario: {
          instructions: "Test instruction",
        },
        initial_state: null,
        evaluation_criteria: {
          actions: [
            {
              action_id: "001_0",
              requestor: "user",
              name: "apply_for_credit_card",
              arguments: { card_type: "Gold", customer_name: "John" },
            },
          ],
          communicate_info: [],
          reward_basis: ["DB", "ACTION", "COMMUNICATE"],
        },
        annotations: null,
      };
      const result = parseSchema(Tau3TaskSchema, task);
      assertRight(result);
      const action = result.right.evaluation_criteria?.actions?.[0];
      expect(action?.name).toBe("apply_for_credit_card");
      expect(action?.arguments).toEqual({
        card_type: "Gold",
        customer_name: "John",
      });
    });
    it("rejects invalid reward_basis enum value", () => {
      const task = {
        id: "task_001",
        description: null,
        user_scenario: {
          instructions: "Test",
        },
        initial_state: null,
        evaluation_criteria: {
          actions: [],
          communicate_info: [],
          reward_basis: ["INVALID_REWARD_TYPE"],
        },
        annotations: null,
      };
      const result = parseSchema(Tau3TaskSchema, task);
      assertLeft(result);
    });
  });
  describe("RewardType enum", () => {
    it("contains all expected reward basis types", () => {
      expect(RewardType.Db).toBe("DB");
      expect(RewardType.EnvAssertion).toBe("ENV_ASSERTION");
      expect(RewardType.NlAssertion).toBe("NL_ASSERTION");
      expect(RewardType.Action).toBe("ACTION");
      expect(RewardType.Communicate).toBe("COMMUNICATE");
    });
  });
  describe("BANKING_TABLES constant", () => {
    it("contains all 23 expected banking table names", () => {
      expect(BANKING_TABLES.length).toBe(23);
      expect(BANKING_TABLES).toContain("users");
      expect(BANKING_TABLES).toContain("accounts");
      expect(BANKING_TABLES).toContain("debit_cards");
      expect(BANKING_TABLES).toContain("credit_card_accounts");
      expect(BANKING_TABLES).toContain("task_config");
      expect(BANKING_TABLES).toContain("agent_discoverable_tools");
      expect(BANKING_TABLES).toContain("debit_card_disputes");
    });
  });
});
