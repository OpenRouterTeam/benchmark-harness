import { describe, expect, it } from "bun:test";

import { compareActionWithToolCall } from "./action-match";
import type { Tau3Action } from "./types";
describe("compareActionWithToolCall", () => {
  it("matches a simple tool call by name and args", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "get_user_information_by_id",
      arguments: { user_id: "user_123" },
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(
      action,
      "get_user_information_by_id",
      {
        user_id: "user_123",
      }
    );
    expect(result).toBe(true);
  });
  it("rejects when tool names do not match", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "get_user_information_by_id",
      arguments: { user_id: "user_123" },
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(
      action,
      "get_user_information_by_name",
      {
        user_id: "user_123",
      }
    );
    expect(result).toBe(false);
  });
  it("rejects when arguments do not match", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "get_user_information_by_id",
      arguments: { user_id: "user_123" },
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(
      action,
      "get_user_information_by_id",
      {
        user_id: "user_456",
      }
    );
    expect(result).toBe(false);
  });
  it("matches inner-tool call for call_discoverable_agent_tool", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "call_discoverable_agent_tool",
      arguments: {
        agent_tool_name: "get_bank_account_transactions_9173",
        arguments: { account_id: "acc_001", limit: 10 },
      },
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(
      action,
      "call_discoverable_agent_tool",
      {
        agent_tool_name: "get_bank_account_transactions_9173",
        arguments: { account_id: "acc_001", limit: 10 },
      }
    );
    expect(result).toBe(true);
  });
  it("rejects inner-tool call when inner tool name differs", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "call_discoverable_agent_tool",
      arguments: {
        agent_tool_name: "get_bank_account_transactions_9173",
        arguments: { account_id: "acc_001" },
      },
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(
      action,
      "call_discoverable_agent_tool",
      {
        agent_tool_name: "get_bank_account_transactions_9174",
        arguments: { account_id: "acc_001" },
      }
    );
    expect(result).toBe(false);
  });
  it("rejects inner-tool call when inner arguments differ", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "call_discoverable_agent_tool",
      arguments: {
        agent_tool_name: "get_bank_account_transactions_9173",
        arguments: { account_id: "acc_001", limit: 10 },
      },
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(
      action,
      "call_discoverable_agent_tool",
      {
        agent_tool_name: "get_bank_account_transactions_9173",
        arguments: { account_id: "acc_001", limit: 20 },
      }
    );
    expect(result).toBe(false);
  });
  it("matches inner-tool call for call_discoverable_user_tool", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "call_discoverable_user_tool",
      arguments: {
        discoverable_tool_name: "apply_for_credit_card",
        arguments: { card_type: "Gold Rewards Card" },
      },
      requestor: "user",
    };
    const result = compareActionWithToolCall(
      action,
      "call_discoverable_user_tool",
      {
        discoverable_tool_name: "apply_for_credit_card",
        arguments: { card_type: "Gold Rewards Card" },
      }
    );
    expect(result).toBe(true);
  });
  it("compares only compare_args keys when compare_args is set", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "transfer_to_human_agents",
      arguments: { reason: "user_request", summary: "golden summary" },
      compare_args: ["reason"],
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(
      action,
      "transfer_to_human_agents",
      {
        reason: "user_request",
        summary: "a completely different summary",
      }
    );
    expect(result).toBe(true);
  });
  it("rejects when a compare_args key differs", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "transfer_to_human_agents",
      arguments: { reason: "user_request" },
      compare_args: ["reason"],
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(
      action,
      "transfer_to_human_agents",
      {
        reason: "out_of_scope",
      }
    );
    expect(result).toBe(false);
  });
  it("matches on name alone when compare_args is empty", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "request_human_agent_transfer",
      arguments: {},
      compare_args: [],
      requestor: "user",
    };
    const result = compareActionWithToolCall(
      action,
      "request_human_agent_transfer",
      {
        anything: "goes",
      }
    );
    expect(result).toBe(true);
  });
  it("matches discoverable call on inner name only when compare_args selects it", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "call_discoverable_agent_tool",
      arguments: {
        agent_tool_name: "get_bank_account_transactions_9173",
        arguments: '{"account_id": "acc_001"}',
      },
      compare_args: ["agent_tool_name"],
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(
      action,
      "call_discoverable_agent_tool",
      {
        agent_tool_name: "get_bank_account_transactions_9173",
        arguments: '{"account_id": "acc_999", "limit": 5}',
      }
    );
    expect(result).toBe(true);
  });
  it("rejects when a compare_args key is present on only one side", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "change_user_email",
      arguments: { user_id: "user_123" },
      compare_args: ["user_id", "new_email"],
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(action, "change_user_email", {
      user_id: "user_123",
      new_email: "a@b.com",
    });
    expect(result).toBe(false);
  });
  it("handles nested object arguments correctly", () => {
    const action: Tau3Action = {
      action_id: "act_001",
      name: "call_discoverable_agent_tool",
      arguments: {
        agent_tool_name: "some_tool",
        arguments: {
          nested: { key1: "value1", key2: 2 },
        },
      },
      requestor: "assistant",
    };
    const result = compareActionWithToolCall(
      action,
      "call_discoverable_agent_tool",
      {
        agent_tool_name: "some_tool",
        arguments: {
          nested: { key2: 2, key1: "value1" },
        },
      }
    );
    expect(result).toBe(true);
  });
});
