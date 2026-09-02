import { beforeEach, describe, expect, it } from "bun:test";

import { Either } from "../../internal/either";
import { parseSchema } from "../../internal/zod";
import {
  applyInitialState,
  dbHash,
  loadBankingData,
  loadBankingTasks,
  makeEmptyBankingData,
  seedBankingCache,
  seedBankingTasksRawCache,
} from "./environment";
import type { BankingData, Tau3Task } from "./types";
import { Tau3TaskSchema } from "./types";
describe("tau3-bench-banking environment", () => {
  const fixtureDb: BankingData = makeEmptyBankingData();
  const FIXTURE_TABLES: Partial<BankingData> = {
    users: {
      data: {
        "123": { name: "Sarah Bosch", user_id: "123" },
      },
    },
    accounts: {
      data: {
        acc_1: { account_id: "acc_1", account_type: "checking" },
      },
    },
    debit_cards: {
      data: {
        card_1: { card_id: "card_1", cardholder: "Sarah Bosch" },
      },
    },
  };
  Object.assign(fixtureDb, FIXTURE_TABLES);
  const fixtureTaskWithoutInitialState: Tau3Task = {
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
          action_id: "001_0",
          requestor: "user",
          name: "apply_for_credit_card",
          arguments: { card_type: "Gold Rewards Card" },
        },
      ],
      communicate_info: [],
      reward_basis: ["DB"],
    },
    annotations: null,
    user_tools: ["apply_for_credit_card"],
    required_documents: ["doc_credit_cards_gold_rewards_card_001"],
  };
  const fixtureTaskWithInitialState: Tau3Task = {
    id: "task_026",
    description: {
      purpose: "Task: task_026",
      relevant_policies: null,
      notes: null,
    },
    user_scenario: {
      persona: null,
      instructions: "You have noticed that your recent cash back rewards...",
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
          action_id: "026_0",
          requestor: "user",
          name: "log_verification",
          arguments: {},
        },
      ],
      communicate_info: [],
      reward_basis: ["DB"],
    },
    annotations: null,
    user_tools: [],
    required_documents: [],
  };
  beforeEach(() => {
    seedBankingCache(fixtureDb, [
      fixtureTaskWithoutInitialState,
      fixtureTaskWithInitialState,
    ]);
  });
  describe("loadBankingData", () => {
    it("returns a mutable copy of the database", () => {
      const data1 = loadBankingData();
      const data2 = loadBankingData();
      expect(data1).not.toBe(data2);
      data1.users.data["999"] = { name: "New User" };
      expect(data2.users.data["999"]).toBeUndefined();
    });
    it("normalizes all 23 tables even if only 3 are in fixture", () => {
      const data = loadBankingData();
      expect(Object.keys(data).length).toBe(23);
      expect(data.users.data["123"]).toEqual({
        name: "Sarah Bosch",
        user_id: "123",
      });
      expect(data.task_config.data).toEqual({});
      expect(data.task_config.notes).toBe("");
    });
    it('fills missing tables with empty data and notes=""', () => {
      const data = loadBankingData();
      expect(data.agent_discoverable_tools).toEqual({ data: {}, notes: "" });
      expect(data.credit_card_orders).toEqual({ data: {}, notes: "" });
    });
  });
  describe("loadBankingTasks", () => {
    it("returns parsed and validated tasks array", () => {
      const tasks = loadBankingTasks();
      expect(tasks.length).toBe(2);
      expect(tasks[0]?.id).toBe("task_001");
      expect(tasks[1]?.id).toBe("task_026");
    });
    it("returns an independent array copy per call", () => {
      const tasks1 = loadBankingTasks();
      const tasks2 = loadBankingTasks();
      expect(tasks1).not.toBe(tasks2);
      tasks1.pop();
      expect(tasks2.length).toBe(2);
    });
    it("validates each task against schema", () => {
      const invalidTask = {
        id: "invalid",
        description: null,
        user_scenario: { instructions: "Test" },
        initial_state: null,
        evaluation_criteria: { reward_basis: ["INVALID"] },
      };
      const invalidParsed = parseSchema(Tau3TaskSchema, invalidTask);
      if (Either.isRight(invalidParsed)) {
        throw new Error("fixture unexpectedly valid");
      }
      seedBankingTasksRawCache(JSON.stringify([invalidTask]));
      expect(() => loadBankingTasks()).toThrow(
        /Invalid banking task at index 0/
      );
    });
    it("returns empty array when seeded with no tasks", () => {
      seedBankingCache(fixtureDb, []);
      const tasks = loadBankingTasks();
      expect(tasks.length).toBe(0);
    });
  });
  describe("applyInitialState", () => {
    it("merges task_config.data into db.task_config.data", () => {
      const db = loadBankingData();
      applyInitialState(db, fixtureTaskWithInitialState);
      expect(db.task_config.data.dispute_settings).toEqual({
        auto_resolve_disputes: true,
      });
    });
    it("deep-merges partial overlays for every banking table", () => {
      const db = loadBankingData();
      const task: Tau3Task = {
        ...fixtureTaskWithoutInitialState,
        initial_state: {
          initialization_data: {
            agent_data: {
              accounts: {
                data: {
                  acc_1: { status: "CLOSED" },
                },
              },
              users: {
                data: {
                  "123": { phone_number: "555-0100" },
                },
              },
            },
          },
          initialization_actions: null,
          message_history: null,
        },
      };
      applyInitialState(db, task);
      expect(db.accounts.data.acc_1).toEqual({
        account_id: "acc_1",
        account_type: "checking",
        status: "CLOSED",
      });
      expect(db.users.data["123"]).toEqual({
        name: "Sarah Bosch",
        user_id: "123",
        phone_number: "555-0100",
      });
    });
    it("rejects overlays for unknown tables", () => {
      const db = loadBankingData();
      const task: Tau3Task = {
        ...fixtureTaskWithoutInitialState,
        initial_state: {
          initialization_data: {
            agent_data: { unknown_table: { data: {} } },
          },
          initialization_actions: null,
          message_history: null,
        },
      };
      expect(() => applyInitialState(db, task)).toThrow(
        "unknown table 'unknown_table'"
      );
    });
    it("does nothing for null initial_state", () => {
      const db = loadBankingData();
      const taskConfigBefore = JSON.stringify(db.task_config.data);
      applyInitialState(db, fixtureTaskWithoutInitialState);
      expect(JSON.stringify(db.task_config.data)).toBe(taskConfigBefore);
    });
    it("does nothing for null initialization_data", () => {
      const db = loadBankingData();
      const task = {
        ...fixtureTaskWithoutInitialState,
        initial_state: {
          initialization_data: null,
          initialization_actions: null,
          message_history: null,
        },
      };
      const taskConfigBefore = JSON.stringify(db.task_config.data);
      applyInitialState(db, task);
      expect(JSON.stringify(db.task_config.data)).toBe(taskConfigBefore);
    });
    it("throws if initialization_actions is non-null", () => {
      const db = loadBankingData();
      const task = {
        ...fixtureTaskWithoutInitialState,
        initial_state: {
          initialization_data: { agent_data: { task_config: { data: {} } } },
          initialization_actions: [{ action: "test" }] satisfies unknown,
          message_history: null,
        },
      };
      expect(() => applyInitialState(db, task)).toThrow(
        /initialization_actions/
      );
    });
    it("throws if message_history is non-null", () => {
      const db = loadBankingData();
      const task = {
        ...fixtureTaskWithoutInitialState,
        initial_state: {
          initialization_data: { agent_data: { task_config: { data: {} } } },
          initialization_actions: null,
          message_history: [{ role: "user" }] satisfies unknown,
        },
      };
      expect(() => applyInitialState(db, task)).toThrow(/message_history/);
    });
    it("hash includes applied task_config", () => {
      const db = loadBankingData();
      const hashBefore = dbHash(db);
      applyInitialState(db, fixtureTaskWithInitialState);
      const hashAfter = dbHash(db);
      expect(hashBefore).not.toBe(hashAfter);
    });
  });
  describe("dbHash", () => {
    it("produces consistent hash for same data", () => {
      const data1 = loadBankingData();
      const hash1 = dbHash(data1);
      const hash2 = dbHash(data1);
      expect(hash1).toBe(hash2);
    });
    it("produces different hash for mutated data", () => {
      const data = loadBankingData();
      const hashBefore = dbHash(data);
      data.users.data["999"] = { name: "New User" };
      const hashAfter = dbHash(data);
      expect(hashBefore).not.toBe(hashAfter);
    });
    it("hashes are 64 hex characters (sha256)", () => {
      const data = loadBankingData();
      const hash = dbHash(data);
      expect(hash).toMatch(/^[a-f0-9]{64}$/);
    });
    it("hash covers injected task_config", () => {
      const data1 = loadBankingData();
      const hash1 = dbHash(data1);
      const data2 = loadBankingData();
      if (data2.task_config) {
        data2.task_config.data.test_key = { value: "test_value" };
      }
      const hash2 = dbHash(data2);
      expect(hash1).not.toBe(hash2);
    });
  });
});
