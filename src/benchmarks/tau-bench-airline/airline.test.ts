import { beforeAll, describe, expect, it } from "bun:test";

import { runSync } from "effect/Effect";

import { MessageRole, ScoreValue } from "../../harness/core";
import { isRecord } from "../../internal/guards";
import { assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { TauBenchAirlineConfigSchema } from "../benchmark-config";
import { benchmarkIds, getBenchmark } from "../registry";
import { compareActionWithToolCall } from "./action-match";
import { airlineRecordToSample, TAU_BENCH_AIRLINE_ID } from "./benchmark";
import { seedAirlineDataCache } from "./environment";
import { evaluateSimulation } from "./evaluator";
import { airlineScorer } from "./scorer";
import { AIRLINE_TOOL_DEFINITIONS } from "./tools/definitions";
import { invokeTool } from "./tools/handlers";
import type { AirlineData, Tau2Task } from "./types";
import { renderUserInstructions } from "./types";

function makeTestData(): AirlineData {
  return {
    flights: {
      HAD001: {
        flight_number: "HAD001",
        origin: "JFK",
        destination: "SFO",
        scheduled_departure_time_est: "08:00",
        scheduled_arrival_time_est: "11:00",
        dates: {
          "2024-05-01": {
            status: "available",
            available_seats: { economy: 50, business: 10 },
            prices: { economy: 200, business: 800 },
          },
        },
      },
    },
    reservations: {
      ABC123: {
        reservation_id: "ABC123",
        user_id: "user_001",
        origin: "JFK",
        destination: "SFO",
        flight_type: "one_way",
        cabin: "economy",
        flights: [
          {
            flight_number: "HAD001",
            date: "2024-05-01",
            price: 200,
            origin: "JFK",
            destination: "SFO",
          },
        ],
        passengers: [{ first_name: "John", last_name: "Doe" }],
        payment_history: [{ payment_id: "credit_001", amount: 200 }],
        created_at: "2024-05-01T10:00:00",
        total_baggages: 1,
        nonfree_baggages: 0,
        insurance: "no",
      },
    },
    users: {
      user_001: {
        name: "John Doe",
        email: "john@example.com",
        payment_methods: {
          credit_001: { source: "credit_card", id: "credit_001", amount: 0 },
        },
        reservations: ["ABC123"],
      },
    },
  };
}

function makeTask(criteria: Tau2Task["evaluation_criteria"]): Tau2Task {
  return {
    id: "t0",
    user_scenario: {
      instructions: {
        domain: "airline",
        reason_for_call: "test",
        task_instructions: "do the thing",
      },
    },
    evaluation_criteria: criteria,
  };
}

const USER_STOP = "USER_STOP";
describe("tau_bench_verified_airline registry", () => {
  it("is registered with correct properties", () => {
    const b = getBenchmark("tau_bench_verified_airline");
    expect(b).toBeDefined();
    expect(b?.id).toBe("tau_bench_verified_airline");
    expect(b?.temperature).toBe(0);
    expect(b?.defaultEpochs).toBe(1);
    expect(b?.userModel).toBe("google/gemini-2.5-flash");
  });
  it("appears in benchmarkIds()", () => {
    expect(benchmarkIds()).toContain("tau_bench_verified_airline");
  });
});
describe("tau_bench_verified_airline config", () => {
  it("defaults the user simulator reasoning effort independently", () => {
    const result = parseSchema(TauBenchAirlineConfigSchema, {
      benchmarkId: "tau_bench_verified_airline",
      model: "openai/gpt-4o-mini",
      reasoningEffort: "low",
    });
    assertRight(result);
    expect(result.right.reasoningEffort).toBe("low");
    expect(result.right.userReasoningEffort).toBe("medium");
  });
  it("allows overriding the user simulator reasoning effort", () => {
    const result = parseSchema(TauBenchAirlineConfigSchema, {
      benchmarkId: "tau_bench_verified_airline",
      model: "openai/gpt-4o-mini",
      reasoningEffort: "low",
      userReasoningEffort: "high",
    });
    assertRight(result);
    expect(result.right.reasoningEffort).toBe("low");
    expect(result.right.userReasoningEffort).toBe("high");
  });
});
describe("airlineRecordToSample", () => {
  it("parses a v2 task_json row and renders instructions as input", () => {
    const task: Tau2Task = makeTask({
      actions: [],
      communicate_info: [],
      nl_assertions: [],
    });
    const sample = airlineRecordToSample(
      { task_json: JSON.stringify(task) },
      0
    );
    expect(sample.id).toBe(`${TAU_BENCH_AIRLINE_ID}-t0`);
    expect(sample.input).toContain("Domain: airline");
    expect(sample.input).toContain("Task instructions:");
  });
  it("throws on a row missing task_json", () => {
    expect(() => airlineRecordToSample({ invalid: true }, 0)).toThrow();
  });
});
describe("renderUserInstructions", () => {
  it("renders required fields, tab-indented, omitting null optionals", () => {
    const rendered = renderUserInstructions({
      domain: "airline",
      reason_for_call: "cancel a flight",
      task_instructions: "be polite",
    });
    expect(rendered).toBe(
      "Domain: airline\nReason for call:\n\tcancel a flight\nTask instructions:\n\tbe polite"
    );
  });
  it("includes known/unknown info when present", () => {
    const rendered = renderUserInstructions({
      domain: "airline",
      reason_for_call: "r",
      known_info: "you are Emma",
      unknown_info: "reservation id",
      task_instructions: "t",
    });
    expect(rendered).toContain("Known info:\n\tyou are Emma");
    expect(rendered).toContain("Unknown info:\n\treservation id");
  });
});
describe("AIRLINE_TOOL_DEFINITIONS", () => {
  it("has 14 tool definitions", () => {
    expect(AIRLINE_TOOL_DEFINITIONS.length).toBe(14);
  });
  it("includes get_flight_status and excludes think", () => {
    const names = AIRLINE_TOOL_DEFINITIONS.map((td) => td.function.name);
    expect(names).toContain("get_flight_status");
    expect(names).not.toContain("think");
  });
  it("send_certificate.amount is an integer", () => {
    const sc = AIRLINE_TOOL_DEFINITIONS.find(
      (td) => td.function.name === "send_certificate"
    );
    const properties = sc?.function.parameters?.["properties"];
    const amount = isRecord(properties) ? properties["amount"] : undefined;
    const amountType = isRecord(amount) ? amount["type"] : undefined;
    expect(amountType).toBe("integer");
  });
});
describe("invokeTool", () => {
  it("get_flight_status returns the date status", () => {
    const data = makeTestData();
    const result = invokeTool(data, "get_flight_status", {
      flight_number: "HAD001",
      date: "2024-05-01",
    });
    expect(result).toBe("available");
  });
  it("cancel_reservation mutates status", () => {
    const data = makeTestData();
    invokeTool(data, "cancel_reservation", { reservation_id: "ABC123" });
    expect(data.reservations["ABC123"]?.status).toBe("cancelled");
  });
  it("returns error for the removed think tool", () => {
    const data = makeTestData();
    expect(invokeTool(data, "think", { thought: "x" })).toContain(
      "Unknown action"
    );
  });
});
describe("compareActionWithToolCall", () => {
  const action = {
    action_id: "a",
    requestor: "assistant",
    name: "cancel_reservation",
    arguments: { reservation_id: "ABC123", note: "x" },
  } as const;
  it("matches all args when compare_args is null", () => {
    expect(
      compareActionWithToolCall(action, "cancel_reservation", {
        reservation_id: "ABC123",
        note: "x",
      })
    ).toBe(true);
    expect(
      compareActionWithToolCall(action, "cancel_reservation", {
        reservation_id: "ABC123",
        note: "y",
      })
    ).toBe(false);
  });
  it("matches name only when compare_args is empty", () => {
    expect(
      compareActionWithToolCall(
        { ...action, compare_args: [] },
        "cancel_reservation",
        {
          anything: 1,
        }
      )
    ).toBe(true);
  });
  it("compares only listed args when compare_args is a subset", () => {
    const a = { ...action, compare_args: ["reservation_id"] };
    expect(
      compareActionWithToolCall(a, "cancel_reservation", {
        reservation_id: "ABC123",
        note: "different",
      })
    ).toBe(true);
  });
  it("fails on name mismatch", () => {
    expect(
      compareActionWithToolCall(action, "book_reservation", {
        reservation_id: "ABC123",
        note: "x",
      })
    ).toBe(false);
  });
});
describe("evaluateSimulation", () => {
  beforeAll(() => {
    seedAirlineDataCache(makeTestData());
  });
  it("scores 0 on premature (MAX_STEPS) termination", () => {
    const task = makeTask({
      actions: [],
      communicate_info: [],
      nl_assertions: [],
    });
    const result = evaluateSimulation({
      task,
      agentData: makeTestData(),
      assistantTexts: [],
      terminationReason: "MAX_STEPS",
    });
    expect(result.reward).toBe(0);
    expect(result.note).toContain("prematurely");
  });
  it("auto-passes nl-assertion-only tasks (no actions, no communicate)", () => {
    const task = makeTask({
      actions: [],
      communicate_info: [],
      nl_assertions: ["Agent should refuse."],
    });
    const result = evaluateSimulation({
      task,
      agentData: makeTestData(),
      assistantTexts: [],
      terminationReason: USER_STOP,
    });
    expect(result.reward).toBe(1);
    expect(result.dbMatch).toBe(true);
  });
  it("empty actions [] still compares DB: agent mutation away from base → reward 0", () => {
    const agentData = makeTestData();
    invokeTool(agentData, "cancel_reservation", { reservation_id: "ABC123" });
    const task = makeTask({
      actions: [],
      communicate_info: [],
      nl_assertions: ["no cancel"],
    });
    const result = evaluateSimulation({
      task,
      agentData,
      assistantTexts: [],
      terminationReason: USER_STOP,
    });
    expect(result.dbMatch).toBe(false);
    expect(result.reward).toBe(0);
  });
  it("absent actions (null) short-circuits to a DB pass even if agent mutated", () => {
    const agentData = makeTestData();
    invokeTool(agentData, "cancel_reservation", { reservation_id: "ABC123" });
    const task = makeTask({ communicate_info: [], nl_assertions: ["x"] });
    const result = evaluateSimulation({
      task,
      agentData,
      assistantTexts: [],
      terminationReason: USER_STOP,
    });
    expect(result.dbMatch).toBe(true);
    expect(result.reward).toBe(1);
  });
  it("DB match: agent end-state equals gold (cancel) → reward 1", () => {
    const agentData = makeTestData();
    invokeTool(agentData, "cancel_reservation", { reservation_id: "ABC123" });
    const task = makeTask({
      actions: [
        {
          action_id: "a1",
          requestor: "assistant",
          name: "cancel_reservation",
          arguments: { reservation_id: "ABC123" },
        },
      ],
      communicate_info: [],
      nl_assertions: [],
    });
    const result = evaluateSimulation({
      task,
      agentData,
      assistantTexts: [],
      terminationReason: USER_STOP,
    });
    expect(result.dbMatch).toBe(true);
    expect(result.reward).toBe(1);
  });
  it("DB mismatch: agent did not perform the gold action → reward 0", () => {
    const agentData = makeTestData();
    const task = makeTask({
      actions: [
        {
          action_id: "a1",
          requestor: "assistant",
          name: "cancel_reservation",
          arguments: { reservation_id: "ABC123" },
        },
      ],
      communicate_info: [],
      nl_assertions: [],
    });
    const result = evaluateSimulation({
      task,
      agentData,
      assistantTexts: [],
      terminationReason: USER_STOP,
    });
    expect(result.dbMatch).toBe(false);
    expect(result.reward).toBe(0);
  });
  it("COMMUNICATE: required info must appear in assistant text (lowercased)", () => {
    const task = makeTask({
      actions: [],
      communicate_info: ["ABC123"],
      nl_assertions: [],
    });
    expect(
      evaluateSimulation({
        task,
        agentData: makeTestData(),
        assistantTexts: ["Your reservation abc123 is set"],
        terminationReason: USER_STOP,
      }).reward
    ).toBe(1);
    expect(
      evaluateSimulation({
        task,
        agentData: makeTestData(),
        assistantTexts: ["no id here"],
        terminationReason: USER_STOP,
      }).reward
    ).toBe(0);
  });
  it("COMMUNICATE: commas are stripped from the agent message, not the expected string", () => {
    const task = makeTask({
      actions: [],
      communicate_info: ["1234"],
      nl_assertions: [],
    });
    expect(
      evaluateSimulation({
        task,
        agentData: makeTestData(),
        assistantTexts: ["Your total is 1,234 dollars"],
        terminationReason: USER_STOP,
      }).reward
    ).toBe(1);
  });
  it("ACTION basis: requires every golden action to be matched by a tool call", () => {
    const task = makeTask({
      actions: [
        {
          action_id: "a1",
          requestor: "assistant",
          name: "cancel_reservation",
          arguments: { reservation_id: "ABC123" },
        },
      ],
      communicate_info: [],
      nl_assertions: [],
      reward_basis: ["ACTION"],
    });
    expect(
      evaluateSimulation({
        task,
        agentData: makeTestData(),
        assistantTexts: [],
        toolCalls: [
          {
            name: "cancel_reservation",
            arguments: { reservation_id: "ABC123" },
          },
        ],
        terminationReason: USER_STOP,
      }).reward
    ).toBe(1);
    expect(
      evaluateSimulation({
        task,
        agentData: makeTestData(),
        assistantTexts: [],
        toolCalls: [
          { name: "get_user_details", arguments: { user_id: "user_001" } },
        ],
        terminationReason: USER_STOP,
      }).reward
    ).toBe(0);
  });
});
describe("airlineScorer", () => {
  beforeAll(() => {
    seedAirlineDataCache(makeTestData());
  });
  it("returns Incorrect when agentData is missing", () => {
    const state = {
      sample: { id: "test", input: "test", target: { text: "" } },
      messages: [],
      completed: true,
    };
    const score = runSync(airlineScorer(state, { text: "" }));
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toContain("Missing");
  });
  it("returns Correct for an nl-only task that ran to USER_STOP", () => {
    const task = makeTask({
      actions: [],
      communicate_info: [],
      nl_assertions: ["x"],
    });
    const state = {
      sample: {
        id: "test",
        input: "test",
        target: { text: "" },
        metadata: {
          agentData: makeTestData(),
          task,
          terminationReason: USER_STOP,
        },
      },
      messages: [{ role: MessageRole.Assistant, content: "ok" }],
      completed: true,
    };
    const score = runSync(airlineScorer(state, { text: "" }));
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.answer).toBe("1");
  });
});
