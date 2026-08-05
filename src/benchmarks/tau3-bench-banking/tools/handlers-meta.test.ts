import { describe, it, expect, beforeEach } from "bun:test";

import { makeEmptyBankingData } from "../environment";
import type { BankingData } from "../types";
import {
  invokeBankingAgentTool,
  registerInitialDiscoverableTools,
} from "./handlers-meta";
import type { BankingEnvState } from "./registry";
import {
  DISCOVERABLE_AGENT_TOOLS,
  DISCOVERABLE_USER_TOOLS,
  makeBankingEnvState,
  registerDiscoverableAgentTool,
} from "./registry";

function createTestDb(): BankingData {
  const db = makeEmptyBankingData();
  db.users.data = {
    usr1: {
      user_id: "usr1",
      name: "Alice Johnson",
      email: "alice@example.com",
      address: "123 Main St",
      phone_number: "555-1234",
      date_of_birth: "01/15/1990",
    },
  };
  db.accounts.data = {
    acc1: {
      account_id: "acc1",
      user_id: "usr1",
      type: "checking",
    },
  };
  return db;
}
describe("handlers-meta", () => {
  let state: BankingEnvState;
  beforeEach(() => {
    const db = createTestDb();
    state = makeBankingEnvState(db);
  });
  describe("invokeBankingAgentTool", () => {
    describe("transfer_to_human_agents", () => {
      it("returns success message for valid transfer", () => {
        const result = invokeBankingAgentTool(
          state,
          "transfer_to_human_agents",
          {
            summary: "User has billing issue",
            reason: "complex_billing_dispute",
          }
        );
        expect(result).toContain("Transfer successful");
        expect(result).toContain("complex_billing_dispute");
      });
      it("returns error for missing summary", () => {
        const result = invokeBankingAgentTool(
          state,
          "transfer_to_human_agents",
          {
            reason: "other",
          }
        );
        expect(result).toContain("Error");
      });
      it("returns error for invalid transfer reason", () => {
        const result = invokeBankingAgentTool(
          state,
          "transfer_to_human_agents",
          {
            summary: "Test",
            reason: "invalid_reason",
          }
        );
        expect(result).toContain("Error: Invalid transfer reason");
      });
      it('defaults to "other" reason if not provided', () => {
        const result = invokeBankingAgentTool(
          state,
          "transfer_to_human_agents",
          {
            summary: "Test issue",
          }
        );
        expect(result).toContain("Transfer successful");
        expect(result).toContain("other");
      });
    });
    describe("get_current_time", () => {
      it("returns frozen timestamp", () => {
        const result = invokeBankingAgentTool(state, "get_current_time", {});
        expect(result).toBe("The current time is 2025-11-14 03:40:00 EST.");
      });
    });
    describe("get_user_information_by_id", () => {
      it("returns user info for valid user_id", () => {
        const result = invokeBankingAgentTool(
          state,
          "get_user_information_by_id",
          {
            user_id: "usr1",
          }
        );
        expect(result).toContain("Found 1 record(s)");
        expect(result).toContain("usr1");
      });
      it("returns error for missing user_id", () => {
        const result = invokeBankingAgentTool(
          state,
          "get_user_information_by_id",
          {}
        );
        expect(result).toContain("Error");
      });
    });
    describe("get_user_information_by_name", () => {
      it("returns user info by name", () => {
        const result = invokeBankingAgentTool(
          state,
          "get_user_information_by_name",
          {
            customer_name: "Alice Johnson",
          }
        );
        expect(result).toContain("Found 1 record(s)");
      });
    });
    describe("get_user_information_by_email", () => {
      it("returns user info by email", () => {
        const result = invokeBankingAgentTool(
          state,
          "get_user_information_by_email",
          {
            email: "alice@example.com",
          }
        );
        expect(result).toContain("Found 1 record(s)");
      });
    });
    describe("change_user_email", () => {
      it("updates email successfully", () => {
        const result = invokeBankingAgentTool(state, "change_user_email", {
          user_id: "usr1",
          new_email: "newemail@example.com",
        });
        expect(result).toContain("Email updated successfully");
        expect(result).toContain("newemail@example.com");
      });
      it("returns error for non-existent user", () => {
        const result = invokeBankingAgentTool(state, "change_user_email", {
          user_id: "usr999",
          new_email: "test@example.com",
        });
        expect(result).toContain("Error: User with ID 'usr999' not found");
      });
    });
    describe("log_verification", () => {
      it("logs verification and writes to db", () => {
        const result = invokeBankingAgentTool(state, "log_verification", {
          name: "Alice Johnson",
          user_id: "usr1",
          address: "123 Main St",
          email: "alice@example.com",
          phone_number: "555-1234",
          date_of_birth: "01/15/1990",
          time_verified: "2025-11-14 03:40:00 EST",
        });
        expect(result).toContain("Verification logged successfully");
        expect(result).toContain("Alice Johnson");
      });
      it("returns error for duplicate verification record", () => {
        const args = {
          name: "Alice Johnson",
          user_id: "usr1",
          address: "123 Main St",
          email: "alice@example.com",
          phone_number: "555-1234",
          date_of_birth: "01/15/1990",
          time_verified: "2025-11-14 03:40:00 EST",
        };
        invokeBankingAgentTool(state, "log_verification", args);
        const result = invokeBankingAgentTool(state, "log_verification", args);
        expect(result).toContain(
          "Failed to log verification: Record may already exist"
        );
      });
    });
    describe("give_discoverable_user_tool", () => {
      beforeEach(() => {
        registerInitialDiscoverableTools();
      });
      it("returns error for unknown tool", () => {
        const result = invokeBankingAgentTool(
          state,
          "give_discoverable_user_tool",
          {
            discoverable_tool_name: "unknown_tool",
          }
        );
        expect(result).toContain(
          "Error: Unknown discoverable tool 'unknown_tool'"
        );
      });
      it("requires discoverable_tool_name parameter", () => {
        const result = invokeBankingAgentTool(
          state,
          "give_discoverable_user_tool",
          {}
        );
        expect(result).toContain("Error");
      });
    });
    describe("unlock_discoverable_agent_tool", () => {
      beforeEach(() => {
        registerInitialDiscoverableTools();
      });
      it("unlocks agent tool and stores in state", () => {
        const result = invokeBankingAgentTool(
          state,
          "unlock_discoverable_agent_tool",
          {
            agent_tool_name: "example_agent_tool_0000",
          }
        );
        expect(result).toContain("Tool unlocked: example_agent_tool_0000");
        expect(state.unlockedAgentTools.has("example_agent_tool_0000")).toBe(
          true
        );
      });
      it("returns error for unknown tool", () => {
        const result = invokeBankingAgentTool(
          state,
          "unlock_discoverable_agent_tool",
          {
            agent_tool_name: "nonexistent_tool",
          }
        );
        expect(result).toContain("Error: Unknown agent tool");
      });
    });
    describe("call_discoverable_agent_tool", () => {
      beforeEach(() => {
        registerInitialDiscoverableTools();
      });
      it("returns error if tool not unlocked", () => {
        const result = invokeBankingAgentTool(
          state,
          "call_discoverable_agent_tool",
          {
            agent_tool_name: "example_agent_tool_0000",
          }
        );
        expect(result).toContain("has not been unlocked");
      });
      it("calls tool after unlock", () => {
        invokeBankingAgentTool(state, "unlock_discoverable_agent_tool", {
          agent_tool_name: "example_agent_tool_0000",
        });
        const result = invokeBankingAgentTool(
          state,
          "call_discoverable_agent_tool",
          {
            agent_tool_name: "example_agent_tool_0000",
          }
        );
        expect(result).toContain("Example tool executed successfully");
      });
      it("writes call record to agent_discoverable_tools table", () => {
        invokeBankingAgentTool(state, "unlock_discoverable_agent_tool", {
          agent_tool_name: "example_agent_tool_0000",
        });
        invokeBankingAgentTool(state, "call_discoverable_agent_tool", {
          agent_tool_name: "example_agent_tool_0000",
        });
        const records = Object.keys(
          state.db.agent_discoverable_tools?.data || {}
        );
        expect(records.length).toBeGreaterThan(0);
      });
      it("logs allowlisted read-only calls once even with different arguments", () => {
        const toolName = "get_all_user_accounts_by_user_id_3847";
        invokeBankingAgentTool(state, "unlock_discoverable_agent_tool", {
          agent_tool_name: toolName,
        });
        invokeBankingAgentTool(state, "call_discoverable_agent_tool", {
          agent_tool_name: toolName,
          arguments: JSON.stringify({ user_id: "usr1" }),
        });
        expect(
          Object.keys(state.db.agent_discoverable_tools?.data ?? {})
        ).toHaveLength(0);
        const allowlistedState = makeBankingEnvState(state.db, [toolName]);
        invokeBankingAgentTool(
          allowlistedState,
          "unlock_discoverable_agent_tool",
          {
            agent_tool_name: toolName,
          }
        );
        invokeBankingAgentTool(
          allowlistedState,
          "call_discoverable_agent_tool",
          {
            agent_tool_name: toolName,
            arguments: JSON.stringify({ user_id: "usr1" }),
          }
        );
        invokeBankingAgentTool(
          allowlistedState,
          "call_discoverable_agent_tool",
          {
            agent_tool_name: toolName,
            arguments: JSON.stringify({ user_id: "usr2" }),
          }
        );
        expect(
          Object.keys(state.db.agent_discoverable_tools?.data ?? {})
        ).toHaveLength(1);
      });
      it("returns handler exceptions as error strings", () => {
        const toolName = "throwing_test_tool";
        registerDiscoverableAgentTool({
          name: toolName,
          description: "Test tool that throws.",
          params: [],
          handler: () => {
            throw new Error("boom");
          },
        });
        try {
          invokeBankingAgentTool(state, "unlock_discoverable_agent_tool", {
            agent_tool_name: toolName,
          });
          expect(
            invokeBankingAgentTool(state, "call_discoverable_agent_tool", {
              agent_tool_name: toolName,
            })
          ).toBe("Error: boom");
        } finally {
          DISCOVERABLE_AGENT_TOOLS.delete(toolName);
        }
      });
    });
    describe("list_discoverable_agent_tools", () => {
      it("returns message when no tools called", () => {
        const result = invokeBankingAgentTool(
          state,
          "list_discoverable_agent_tools",
          {}
        );
        expect(result).toContain("No agent tools have been called yet");
      });
    });
    describe("transfer tools", () => {
      beforeEach(() => {
        registerInitialDiscoverableTools();
      });
      it("initial_transfer_to_human_agent_0218 returns expected message", () => {
        invokeBankingAgentTool(state, "unlock_discoverable_agent_tool", {
          agent_tool_name: "initial_transfer_to_human_agent_0218",
        });
        const result = invokeBankingAgentTool(
          state,
          "call_discoverable_agent_tool",
          {
            agent_tool_name: "initial_transfer_to_human_agent_0218",
          }
        );
        expect(result).toContain("Initial Transfer Initiated");
        expect(result).toContain("lines are busy");
      });
      it("initial_transfer_to_human_agent_1822 returns expected message", () => {
        invokeBankingAgentTool(state, "unlock_discoverable_agent_tool", {
          agent_tool_name: "initial_transfer_to_human_agent_1822",
        });
        const result = invokeBankingAgentTool(
          state,
          "call_discoverable_agent_tool",
          {
            agent_tool_name: "initial_transfer_to_human_agent_1822",
          }
        );
        expect(result).toContain(
          "conversation has not reached the necessary stages"
        );
      });
      it("emergency_credit_bureau_incident_transfer_1114 returns expected message", () => {
        invokeBankingAgentTool(state, "unlock_discoverable_agent_tool", {
          agent_tool_name: "emergency_credit_bureau_incident_transfer_1114",
        });
        const result = invokeBankingAgentTool(
          state,
          "call_discoverable_agent_tool",
          {
            agent_tool_name: "emergency_credit_bureau_incident_transfer_1114",
          }
        );
        expect(result).toContain("Emergency escalation logged");
        expect(result).toContain("credit bureau correction team");
      });
      it("does not log generic transfer tools", () => {
        const toolNames = [
          "initial_transfer_to_human_agent_0218",
          "initial_transfer_to_human_agent_1822",
          "emergency_credit_bureau_incident_transfer_1114",
        ];
        for (const agentToolName of toolNames) {
          invokeBankingAgentTool(state, "unlock_discoverable_agent_tool", {
            agent_tool_name: agentToolName,
          });
          invokeBankingAgentTool(state, "call_discoverable_agent_tool", {
            agent_tool_name: agentToolName,
          });
        }
        expect(
          Object.keys(state.db.agent_discoverable_tools?.data ?? {})
        ).toHaveLength(0);
      });
    });
    describe("unknown tool", () => {
      it("returns error for unknown tool name", () => {
        const result = invokeBankingAgentTool(
          state,
          "unknown_tool" satisfies unknown satisfies unknown,
          {}
        );
        expect(result).toContain("Error: Unknown tool 'unknown_tool'");
      });
    });
  });
  describe("registerInitialDiscoverableTools", () => {
    it("registers example agent tool", () => {
      registerInitialDiscoverableTools();
      invokeBankingAgentTool(state, "unlock_discoverable_agent_tool", {
        agent_tool_name: "example_agent_tool_0000",
      });
      expect(state.unlockedAgentTools.has("example_agent_tool_0000")).toBe(
        true
      );
    });
    it("registers all three transfer tools", () => {
      registerInitialDiscoverableTools();
      const tools = [
        "initial_transfer_to_human_agent_0218",
        "initial_transfer_to_human_agent_1822",
        "emergency_credit_bureau_incident_transfer_1114",
      ];
      for (const tool of tools) {
        invokeBankingAgentTool(state, "unlock_discoverable_agent_tool", {
          agent_tool_name: tool,
        });
        expect(state.unlockedAgentTools.has(tool)).toBe(true);
      }
    });
    it("registers discoverable tools from every banking domain", () => {
      registerInitialDiscoverableTools();
      expect(DISCOVERABLE_AGENT_TOOLS.has("open_bank_account_4821")).toBe(true);
      expect(DISCOVERABLE_AGENT_TOOLS.has("apply_statement_credit_8472")).toBe(
        true
      );
      expect(DISCOVERABLE_AGENT_TOOLS.has("activate_debit_card_8292")).toBe(
        true
      );
      expect(
        DISCOVERABLE_AGENT_TOOLS.has(
          "file_credit_card_transaction_dispute_4829"
        )
      ).toBe(true);
      expect(DISCOVERABLE_USER_TOOLS.has("deposit_check_3847")).toBe(true);
    });
  });
});
