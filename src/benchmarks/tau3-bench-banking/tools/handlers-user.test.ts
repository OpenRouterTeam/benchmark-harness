import { describe, it, expect, beforeEach } from "bun:test";

import { makeEmptyBankingData } from "../environment";
import type { BankingData } from "../types";
import { addToDb } from "./db-query";
import { invokeBankingUserTool, registerUserTools } from "./handlers-user";
import { generateUserDiscoverableToolId } from "./ids";
import type { BankingEnvState } from "./registry";
import { makeBankingEnvState } from "./registry";

function createTestDb(): BankingData {
  return {
    ...makeEmptyBankingData(),
    accounts: {
      data: {
        acc_1: {
          account_id: "acc_1",
          status: "ACTIVE",
          current_holdings: "$1000.00",
          balance: 1000,
        },
      },
    },
    credit_card_accounts: {
      data: {
        cc_gold_1: {
          account_id: "cc_gold_1",
          user_id: "user_1",
          card_type: "Gold Rewards Card",
        },
      },
    },
    task_config: {
      data: { dispute_settings: { auto_resolve_disputes: false } },
    },
  };
}

function giveTool(
  db: BankingData,
  state: BankingEnvState,
  toolName: string
): void {
  const toolId = generateUserDiscoverableToolId(toolName);
  addToDb({
    db,
    dbName: "user_discoverable_tools",
    recordId: toolId,
    record: {
      tool_name: toolName,
      status: "GIVEN",
    },
  });
  state.givenUserTools.set(toolName, {});
}
describe("handlers-user", () => {
  beforeEach(() => {
    registerUserTools();
  });
  describe("getReferralLink (discoverable tool)", () => {
    it("returns error when tool not given", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "get_referral_link",
          arguments: JSON.stringify({
            user_id: "user_1",
            card_name: "Gold Rewards Card",
          }),
        }
      );
      expect(result).toContain(
        "Error: Tool 'get_referral_link' has not been given"
      );
    });
    it("generates a referral link when tool given", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      giveTool(db, state, "get_referral_link");
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "get_referral_link",
          arguments: JSON.stringify({
            user_id: "user_1",
            card_name: "Gold Rewards Card",
          }),
        }
      );
      expect(result).toContain("Referral link generated successfully");
      expect(result).toContain("Referral ID:");
      expect(result).toContain("https://rhobank.com/refer/");
      expect(Object.keys(db.user_discoverable_tool_calls!.data).length).toBe(1);
    });
    it("returns error when missing user_id", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      giveTool(db, state, "get_referral_link");
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "get_referral_link",
          arguments: JSON.stringify({ card_name: "Gold Rewards Card" }),
        }
      );
      expect(result).toContain("Error: Missing required parameters");
    });
  });
  describe("getCardLast4Digits (discoverable tool)", () => {
    it("returns error when tool not given", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "get_card_last_4_digits",
          arguments: JSON.stringify({ credit_card_account_id: "cc_gold_1" }),
        }
      );
      expect(result).toContain(
        "Error: Tool 'get_card_last_4_digits' has not been given"
      );
    });
    it("returns last 4 digits for a valid card when tool given", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      giveTool(db, state, "get_card_last_4_digits");
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "get_card_last_4_digits",
          arguments: JSON.stringify({ credit_card_account_id: "cc_gold_1" }),
        }
      );
      expect(result).toContain("Card information retrieved successfully");
      expect(result).toContain("Last 4 digits of card:");
      expect(Object.keys(db.user_discoverable_tool_calls!.data).length).toBe(1);
    });
    it("returns error for non-existent card", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      giveTool(db, state, "get_card_last_4_digits");
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "get_card_last_4_digits",
          arguments: JSON.stringify({ credit_card_account_id: "cc_nonexist" }),
        }
      );
      expect(result).toContain("Error");
      expect(result).toContain("not found");
    });
  });
  describe("applyForCreditCard", () => {
    it("submits a credit card application", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "apply_for_credit_card", {
        card_type: "Gold Rewards Card",
        customer_name: "John Doe",
        annual_income: 75000,
        rho_bank_subscription: false,
      });
      expect(result).toContain("Credit card application submitted");
      expect(result).toContain("5-7 business days");
    });
    it("rejects invalid card type", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "apply_for_credit_card", {
        card_type: "Invalid Card",
        customer_name: "John Doe",
        annual_income: 75000,
      });
      expect(result).toContain("Error: Invalid card_type");
    });
    it("returns error when missing parameters", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "apply_for_credit_card", {
        card_type: "Gold Rewards Card",
      });
      expect(result).toContain("Error: Missing required parameters");
    });
  });
  describe("submitReferral", () => {
    it("submits a referral", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "submit_referral", {
        user_id: "user_1",
        account_type: "Gold Rewards Card",
      });
      expect(result).toContain("Referral request submitted successfully");
      expect(result).toContain("Referral ID:");
      expect(result).toContain("NO_PROGRESS");
    });
    it("returns error when missing parameters", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "submit_referral", {
        user_id: "user_1",
      });
      expect(result).toContain("Error: Missing required parameters");
    });
  });
  describe("queryDatabase", () => {
    it("queries a database with constraints", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "query_database", {
        database_name: "accounts",
        constraints: "{}",
      });
      expect(result).toContain("Found");
      expect(result).toContain("acc_1");
    });
    it("returns error for non-existent database", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "query_database", {
        database_name: "nonexistent_db",
      });
      expect(result).toContain("Error");
      expect(result).toContain("not found");
    });
  });
  describe("submitTransaction", () => {
    it("submits a transaction", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "submit_transaction", {
        user_id: "user_1",
        credit_card_type: "Gold Rewards Card",
        merchant_name: "Amazon",
        amount: 100.5,
        category: "Shopping",
      });
      expect(result).toContain("Transaction submitted successfully");
      expect(result).toContain("user_1");
      expect(result).toContain("Gold Rewards Card");
      expect(result).toContain("100.50");
    });
    it("returns error for invalid credit card type", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "submit_transaction", {
        user_id: "user_1",
        credit_card_type: "Invalid Card",
        merchant_name: "Amazon",
        amount: 100,
        category: "Shopping",
      });
      expect(result).toContain("Error");
      expect(result).toContain("Unknown credit card type");
    });
    it("returns error for negative amount", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "submit_transaction", {
        user_id: "user_1",
        credit_card_type: "Gold Rewards Card",
        merchant_name: "Amazon",
        amount: -50,
        category: "Shopping",
      });
      expect(result).toContain("Error");
      expect(result).toContain("positive number");
    });
  });
  describe("requestHumanAgentTransfer", () => {
    it("submits a transfer request", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(
        state,
        "request_human_agent_transfer",
        {}
      );
      expect(result).toContain("Transfer request #1 submitted");
      expect(result).toContain("will process your request");
    });
    it("logs transfer request to database", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      invokeBankingUserTool(state, "request_human_agent_transfer", {});
      expect(
        Object.keys(db.human_transfer_requests!.data).length
      ).toBeGreaterThan(0);
    });
  });
  describe("submitCashBackDispute (discoverable tool)", () => {
    it("returns error when tool not given", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "submit_cash_back_dispute_0589",
          arguments: JSON.stringify({
            user_id: "user_1",
            transaction_id: "txn_1",
          }),
        }
      );
      expect(result).toContain(
        "Error: Tool 'submit_cash_back_dispute_0589' has not been given"
      );
    });
    it("submits dispute when tool given", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const toolId = generateUserDiscoverableToolId(
        "submit_cash_back_dispute_0589"
      );
      addToDb({
        db,
        dbName: "user_discoverable_tools",
        recordId: toolId,
        record: {
          tool_name: "submit_cash_back_dispute_0589",
          status: "GIVEN",
        },
      });
      state.givenUserTools.set("submit_cash_back_dispute_0589", {});
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "submit_cash_back_dispute_0589",
          arguments: JSON.stringify({
            user_id: "user_1",
            transaction_id: "txn_1",
          }),
        }
      );
      expect(result).toContain("Cash back dispute submitted successfully");
      expect(result).toContain("Dispute ID:");
    });
    it("logs transaction to user_discoverable_tool_calls", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const toolId = generateUserDiscoverableToolId(
        "submit_cash_back_dispute_0589"
      );
      addToDb({
        db,
        dbName: "user_discoverable_tools",
        recordId: toolId,
        record: {
          tool_name: "submit_cash_back_dispute_0589",
          status: "GIVEN",
        },
      });
      state.givenUserTools.set("submit_cash_back_dispute_0589", {});
      invokeBankingUserTool(state, "call_discoverable_user_tool", {
        discoverable_tool_name: "submit_cash_back_dispute_0589",
        arguments: JSON.stringify({
          user_id: "user_1",
          transaction_id: "txn_1",
        }),
      });
      const callsData = db.user_discoverable_tool_calls?.data ?? {};
      expect(Object.keys(callsData).length).toBeGreaterThan(0);
      const call = Object.values(callsData)[0] as
        | Record<string, unknown>
        | undefined;
      expect(call?.tool_name).toBe("submit_cash_back_dispute_0589");
      expect(call?.status).toBe("CALLED");
    });
  });
  describe("depositCheck (discoverable tool)", () => {
    it("returns error when tool not given", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "deposit_check_3847",
          arguments: JSON.stringify({ account_id: "acc_1", check_amount: 500 }),
        }
      );
      expect(result).toContain(
        "Error: Tool 'deposit_check_3847' has not been given"
      );
    });
    it("deposits check when tool given", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const toolId = generateUserDiscoverableToolId("deposit_check_3847");
      addToDb({
        db,
        dbName: "user_discoverable_tools",
        recordId: toolId,
        record: {
          tool_name: "deposit_check_3847",
          status: "GIVEN",
        },
      });
      state.givenUserTools.set("deposit_check_3847", {});
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "deposit_check_3847",
          arguments: JSON.stringify({ account_id: "acc_1", check_amount: 500 }),
        }
      );
      expect(result).toContain("Check deposited successfully");
      expect(result).toContain("Previous Balance: $1000.00");
      expect(result).toContain("New Balance: $1500.00");
    });
    it("updates account balance correctly", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const toolId = generateUserDiscoverableToolId("deposit_check_3847");
      addToDb({
        db,
        dbName: "user_discoverable_tools",
        recordId: toolId,
        record: {
          tool_name: "deposit_check_3847",
          status: "GIVEN",
        },
      });
      state.givenUserTools.set("deposit_check_3847", {});
      invokeBankingUserTool(state, "call_discoverable_user_tool", {
        discoverable_tool_name: "deposit_check_3847",
        arguments: JSON.stringify({
          account_id: "acc_1",
          check_amount: 250.75,
        }),
      });
      const account = db.accounts!.data.acc_1 as
        | Record<string, unknown>
        | undefined;
      expect(account?.current_holdings).toBe("$1250.75");
    });
  });
  describe("listDiscoverableUserTools", () => {
    it("returns empty message when no tools given", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(
        state,
        "list_discoverable_user_tools",
        {}
      );
      expect(result).toContain("No tools have been given");
    });
    it("lists given tools", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const toolId = generateUserDiscoverableToolId("deposit_check_3847");
      addToDb({
        db,
        dbName: "user_discoverable_tools",
        recordId: toolId,
        record: {
          tool_name: "deposit_check_3847",
          status: "GIVEN",
        },
      });
      const result = invokeBankingUserTool(
        state,
        "list_discoverable_user_tools",
        {}
      );
      expect(result).toContain("Tools given to you");
      expect(result).toContain("deposit_check_3847");
    });
  });
  describe("callDiscoverableUserTool", () => {
    it("returns error for unknown tool", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "unknown_tool_xyz",
          arguments: "{}",
        }
      );
      expect(result).toContain(
        "Error: Unknown discoverable tool 'unknown_tool_xyz'"
      );
    });
    it("parses JSON arguments correctly", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const toolId = generateUserDiscoverableToolId("deposit_check_3847");
      addToDb({
        db,
        dbName: "user_discoverable_tools",
        recordId: toolId,
        record: {
          tool_name: "deposit_check_3847",
          status: "GIVEN",
        },
      });
      state.givenUserTools.set("deposit_check_3847", {});
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "deposit_check_3847",
          arguments: '{"account_id": "acc_1", "check_amount": 100}',
        }
      );
      expect(result).toContain("Check deposited successfully");
    });
    it("returns error for malformed JSON", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(
        state,
        "call_discoverable_user_tool",
        {
          discoverable_tool_name: "deposit_check_3847",
          arguments: "{invalid json}",
        }
      );
      expect(result).toContain("Error: Invalid JSON");
    });
  });
  describe("invokeBankingUserTool dispatcher", () => {
    it("returns error for unknown tool name", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const result = invokeBankingUserTool(state, "unknown_tool", {});
      expect(result).toContain("Error: Unknown user tool");
    });
  });
});
