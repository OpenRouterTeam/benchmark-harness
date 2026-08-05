import { describe, it, expect, beforeEach } from "bun:test";

import { makeEmptyBankingData } from "../environment";
import type { BankingData } from "../types";
import { registerAccountTools } from "./handlers-accounts";
import { makeBankingEnvState, DISCOVERABLE_AGENT_TOOLS } from "./registry";

function createTestDb(): BankingData {
  return {
    ...makeEmptyBankingData(),
    users: {
      data: {
        user_123: {
          user_id: "user_123",
          name: "Alice Johnson",
        },
        user_456: {
          user_id: "user_456",
          name: "Bob Smith",
        },
      },
    },
    accounts: {
      data: {
        acc_001: {
          account_id: "acc_001",
          user_id: "user_123",
          class: "checking",
          account_type: "checking",
          level: "Blue Account",
          status: "OPEN",
          date_opened: "10/01/2025",
          current_holdings: "1000.00",
        },
        acc_002: {
          account_id: "acc_002",
          user_id: "user_123",
          class: "saving",
          account_type: "savings",
          level: "Bronze Account",
          status: "OPEN",
          date_opened: "09/01/2025",
          current_holdings: "500.00",
        },
        acc_003: {
          account_id: "acc_003",
          user_id: "user_456",
          class: "checking",
          account_type: "checking",
          level: "Green Account",
          status: "OPEN",
          date_opened: "08/15/2023",
          current_holdings: "5000.00",
        },
        acc_004: {
          account_id: "acc_004",
          user_id: "user_456",
          class: "saving",
          account_type: "savings",
          level: "Silver Account",
          status: "OPEN",
          date_opened: "08/15/2023",
          current_holdings: "2000.00",
        },
      },
    },
  };
}
describe("handlers-accounts", () => {
  beforeEach(() => {
    (DISCOVERABLE_AGENT_TOOLS satisfies unknown).clear();
    registerAccountTools();
  });
  describe("open_bank_account_4821", () => {
    it("should create a new checking account successfully", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "open_bank_account_4821"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        user_id: "user_123",
        account_type: "checking",
        account_class: "Standard Checking",
      });
      expect(result).toContain("Bank account opened successfully!");
      expect(result).toContain("Account Type: checking");
      expect(result).toContain("Status: OPEN");
      expect(result).toContain("Initial Balance: $0.00");
    });
    it("should reject invalid account_type", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "open_bank_account_4821"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        user_id: "user_123",
        account_type: "invalid_type",
        account_class: "Test",
      });
      expect(result).toContain("Error: Invalid account_type");
    });
    it("should reject savings without eligible checking account", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      db.accounts.data["acc_new"] = {
        account_id: "acc_new",
        user_id: "user_999",
        class: "checking",
        account_type: "checking",
        status: "OPEN",
        date_opened: "11/10/2025",
        current_holdings: "0",
      };
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "open_bank_account_4821"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        user_id: "user_999",
        account_type: "savings",
        account_class: "Savings",
      });
      expect(result).toContain(
        "Error: Account eligibility requirements not met"
      );
    });
    it("should reject business_checking without personal checking", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "open_bank_account_4821"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        user_id: "user_new",
        account_type: "business_checking",
        account_class: "Business",
      });
      expect(result).toContain(
        "Error: Account eligibility requirements not met"
      );
    });
    it("should reject business_savings without business_checking", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "open_bank_account_4821"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        user_id: "user_123",
        account_type: "business_savings",
        account_class: "Business Savings",
      });
      expect(result).toContain(
        "Error: Account eligibility requirements not met"
      );
    });
    it("should reject with missing parameters", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "open_bank_account_4821"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        user_id: "",
        account_type: "checking",
      });
      expect(result).toContain("Error: Missing required parameters");
    });
  });
  describe("transfer_funds_between_bank_accounts_7291", () => {
    it("should transfer funds successfully and update both balances", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "transfer_funds_between_bank_accounts_7291"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        source_account_id: "acc_001",
        destination_account_id: "acc_002",
        amount: 100,
      });
      expect(result).toContain("Transfer completed successfully!");
      expect(result).toContain("Amount: $100.00");
      expect(result).toContain("new balance: $900.00");
      expect(result).toContain("new balance: $600.00");
      const source = db.accounts.data["acc_001"] as Record<string, unknown>;
      const dest = db.accounts.data["acc_002"] as Record<string, unknown>;
      expect(source.current_holdings).toBe("$900.00");
      expect(dest.current_holdings).toBe("$600.00");
    });
    it("should reject transfer with insufficient funds", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "transfer_funds_between_bank_accounts_7291"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        source_account_id: "acc_001",
        destination_account_id: "acc_002",
        amount: 2000,
      });
      expect(result).toContain("Error: Insufficient funds");
      expect(result).toContain("$1000.00");
      expect(result).toContain("$2000.00");
    });
    it("should reject transfer from non-existent account", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "transfer_funds_between_bank_accounts_7291"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        source_account_id: "acc_nonexistent",
        destination_account_id: "acc_002",
        amount: 100,
      });
      expect(result).toContain(
        "Error: Source account 'acc_nonexistent' not found"
      );
    });
    it("should reject transfer from same account to itself", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "transfer_funds_between_bank_accounts_7291"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        source_account_id: "acc_001",
        destination_account_id: "acc_001",
        amount: 100,
      });
      expect(result).toContain(
        "Error: Source and destination accounts cannot be the same"
      );
    });
    it("should reject negative transfer amount", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "transfer_funds_between_bank_accounts_7291"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        source_account_id: "acc_001",
        destination_account_id: "acc_002",
        amount: -50,
      });
      expect(result).toContain("Error: Transfer amount must be positive");
    });
  });
  describe("apply_checking_account_credit_5829", () => {
    it("should apply credit successfully and create transaction record", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_checking_account_credit_5829"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_001",
        amount: 50,
        credit_type: "rebate_credit",
      });
      expect(result).toContain("Credit applied successfully!");
      expect(result).toContain("Credit Type: rebate_credit");
      expect(result).toContain("Amount: $50.00");
      expect(result).toContain("Previous Balance: $1000.00");
      expect(result).toContain("New Balance: $1050.00");
      const account = db.accounts.data["acc_001"] as Record<string, unknown>;
      expect(account.current_holdings).toBe("$1050.00");
      const txnKeys = Object.keys(db.bank_account_transaction_history.data);
      expect(txnKeys.length).toBeGreaterThan(0);
      const txnId = txnKeys[0];
      if (!txnId) {
        throw new Error("Expected transaction ID");
      }
      const txn = db.bank_account_transaction_history.data[txnId] as Record<
        string,
        unknown
      >;
      expect(txn.account_id).toBe("acc_001");
      expect(txn.type).toBe("rebate_credit");
    });
    it("should reject invalid credit_type", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_checking_account_credit_5829"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_001",
        amount: 50,
        credit_type: "invalid_credit",
      });
      expect(result).toContain("Error: Invalid credit_type");
    });
    it("should reject a non-numeric amount without touching the balance", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_checking_account_credit_5829"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_001",
        amount: "abc",
        credit_type: "rebate_credit",
      });
      expect(result).toBe("Error: Invalid credit amount. Must be a number.");
      const account = db.accounts.data["acc_001"] as Record<string, unknown>;
      expect(account.current_holdings).toBe("1000.00");
    });
    it("should reject credit on non-checking account", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_checking_account_credit_5829"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_002",
        amount: 50,
        credit_type: "rebate_credit",
      });
      expect(result).toContain("Error: Account");
      expect(result).toContain("is not a checking account");
    });
  });
  describe("apply_savings_account_credit_6831", () => {
    it("should reject a non-numeric amount without touching the balance", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_savings_account_credit_6831"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_002",
        amount: "abc",
        credit_type: "interest_correction",
      });
      expect(result).toBe("Error: Invalid credit amount. Must be a number.");
      const account = db.accounts.data["acc_002"] as Record<string, unknown>;
      expect(account.current_holdings).toBe("500.00");
    });
    it("should apply savings credit successfully", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_savings_account_credit_6831"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_002",
        amount: 25.5,
        credit_type: "interest_correction",
      });
      expect(result).toContain("Credit applied successfully!");
      expect(result).toContain("Credit Type: interest_correction");
      expect(result).toContain("Amount: $25.50");
      expect(result).toContain("Previous Balance: $500.00");
      expect(result).toContain("New Balance: $525.50");
      const account = db.accounts.data["acc_002"] as Record<string, unknown>;
      expect(account.current_holdings).toBe("525.50");
    });
    it("should reject positive amount requirement", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_savings_account_credit_6831"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_002",
        amount: 0,
        credit_type: "interest_correction",
      });
      expect(result).toContain("Error: Credit amount must be positive");
    });
    it("should reject credit on non-savings account", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_savings_account_credit_6831"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_001",
        amount: 25,
        credit_type: "interest_correction",
      });
      expect(result).toContain("Error: Account");
      expect(result).toContain("is not a savings account");
    });
  });
  describe("submit_interest_discrepancy_report_7294", () => {
    it("should submit interest discrepancy report successfully", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "submit_interest_discrepancy_report_7294"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_002",
        user_id: "user_123",
        expected_apy: 2.775,
        actual_apy: 2.5,
        amount_difference: 6.88,
      });
      expect(result).toContain(
        "Interest Discrepancy Report Submitted Successfully!"
      );
      expect(result).toContain("Expected APY: 2.775%");
      expect(result).toContain("Actual APY: 2.5%");
      expect(result).toContain("Amount Difference: $6.88");
      expect(result).toContain("Status: PENDING_REVIEW");
    });
    it("should reject non-existent account", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "submit_interest_discrepancy_report_7294"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_nonexistent",
        user_id: "user_123",
        expected_apy: 2.775,
        actual_apy: 2.5,
        amount_difference: 6.88,
      });
      expect(result).toContain("Error: Account 'acc_nonexistent' not found");
    });
    it("should reject non-existent user", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "submit_interest_discrepancy_report_7294"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_002",
        user_id: "user_nonexistent",
        expected_apy: 2.775,
        actual_apy: 2.5,
        amount_difference: 6.88,
      });
      expect(result).toContain("Error: User 'user_nonexistent' not found");
    });
    it("should handle APY difference rounding correctly", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "submit_interest_discrepancy_report_7294"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_002",
        user_id: "user_123",
        expected_apy: 2.7753,
        actual_apy: 2.5001,
        amount_difference: 6.88,
      });
      expect(result).toContain("APY Difference: 0.2752%");
    });
  });
  describe("close_bank_account_7392", () => {
    it("should close account with zero balance successfully", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      (
        db.accounts.data["acc_001"] as Record<string, unknown>
      ).current_holdings = "0.00";
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "close_bank_account_7392"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_001",
        reason: "Customer request",
        waive_early_closure_fee: true,
      });
      expect(result).toContain("Bank account closed successfully!");
      expect(result).toContain("Status: CLOSED");
      expect(result).toContain("Early Closure Fee Waived: Yes");
      const account = db.accounts.data["acc_001"] as Record<string, unknown>;
      expect(account.status).toBe("CLOSED");
    });
    it("should reject closing already-closed account", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      (db.accounts.data["acc_001"] as Record<string, unknown>).status =
        "CLOSED";
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "close_bank_account_7392"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_001",
        reason: "Test",
      });
      expect(result).toContain("Error: Account");
      expect(result).toContain("is already closed");
    });
    it("should reject closing account with non-zero balance", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "close_bank_account_7392"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_001",
        reason: "Test",
        waive_early_closure_fee: true,
      });
      expect(result).toContain("Error: Account balance must be $0.00");
      expect(result).toContain("$1000.00");
    });
    it("should reject with missing account_id", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "close_bank_account_7392"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, { reason: "Test" });
      expect(result).toContain("Error: Missing required parameter");
    });
  });
  describe("get_all_user_accounts_by_user_id_3847", () => {
    it("should return all accounts for user", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "get_all_user_accounts_by_user_id_3847"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        user_id: "user_123",
      });
      expect(result).toContain("User accounts retrieved successfully");
      expect(result).toContain("Bank Accounts:");
      expect(result).toContain("Credit Card Accounts:");
    });
    it("should reject with missing user_id", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "get_all_user_accounts_by_user_id_3847"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {});
      expect(result).toContain("Error: Missing required parameter: user_id");
    });
  });
  describe("get_bank_account_transactions_9173", () => {
    it("should return transactions for account", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      db.bank_account_transaction_history.data["txn_001"] = {
        transaction_id: "txn_001",
        account_id: "acc_001",
        date: "11/14/2025",
        description: "Test transaction",
        amount: 100,
        type: "credit",
        status: "posted",
      };
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "get_bank_account_transactions_9173"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_001",
      });
      expect(result).toContain(
        "Bank account transactions retrieved successfully"
      );
      expect(result).toContain("Transactions for account acc_001");
      expect(result).toContain(
        "Found 1 record(s) in 'bank_account_transaction_history':"
      );
      expect(result).toContain("1. Record ID: txn_001");
    });
    it("should return transactions in reverse chronological order", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      db.bank_account_transaction_history.data = {
        older: {
          transaction_id: "older",
          account_id: "acc_001",
          date: "11/13/2025",
          description: "Older",
        },
        newer: {
          transaction_id: "newer",
          account_id: "acc_001",
          date: "11/14/2025 03:40:00",
          description: "Newer",
        },
      };
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "get_bank_account_transactions_9173"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, { account_id: "acc_001" });
      expect(result.indexOf("Record ID: newer")).toBeLessThan(
        result.indexOf("Record ID: older")
      );
    });
    it("should reject non-existent account", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "get_bank_account_transactions_9173"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {
        account_id: "acc_nonexistent",
      });
      expect(result).toContain("Error: Account 'acc_nonexistent' not found");
    });
    it("should reject with missing account_id", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      const handler = DISCOVERABLE_AGENT_TOOLS.get(
        "get_bank_account_transactions_9173"
      )?.handler;
      if (!handler) {
        throw new Error("Tool not registered");
      }
      const result = handler(state, {});
      expect(result).toContain("Error: Missing required parameter: account_id");
    });
  });
});
