import { describe, it, expect, beforeEach } from "bun:test";

import { makeEmptyBankingData } from "../environment";
import type { BankingData } from "../types";
import { registerCreditTools } from "./handlers-credit";
import type { BankingEnvState } from "./registry";
import { makeBankingEnvState, DISCOVERABLE_AGENT_TOOLS } from "./registry";

function makeTestDb(): BankingData {
  return {
    ...makeEmptyBankingData(),
    users: {
      data: {
        user_123: {
          user_id: "user_123",
          name: "John Doe",
          email: "john@example.com",
        },
      },
    },
    accounts: {
      data: {
        checking_001: {
          account_id: "checking_001",
          user_id: "user_123",
          class: "checking",
          balance: "5000.00",
          current_holdings: "5000.00",
        },
      },
    },
    credit_card_accounts: {
      data: {
        cc_001: {
          account_id: "cc_001",
          user_id: "user_123",
          credit_limit: "$5000.00",
          current_balance: "$1500.00",
          account_status: "ACTIVE",
          status: "ACTIVE",
        },
      },
    },
    payment_history: {
      data: {
        payment_001: {
          payment_id: "payment_001",
          credit_card_account_id: "cc_001",
          payment_date: "11/10/2025",
          amount: "$500.00",
          status: "ON_TIME",
        },
        payment_002: {
          payment_id: "payment_002",
          credit_card_account_id: "cc_001",
          payment_date: "11/03/2025",
          amount: "$500.00",
          status: "ON_TIME",
        },
      },
    },
  };
}
describe("handlers-credit", () => {
  let state: BankingEnvState;
  beforeEach(() => {
    state = makeBankingEnvState(makeTestDb());
    registerCreditTools();
  });
  describe("order_replacement_credit_card_7291", () => {
    it("should order replacement card successfully", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "order_replacement_credit_card_7291"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        shipping_address: "123 Main St, Boston, MA 02101",
        reason: "lost",
        expedited_shipping: false,
      });
      expect(result).toContain("Order ID:");
      expect(result).toContain("Card Account: cc_001");
      expect(result).toContain("Reason: Lost");
      expect(result).toContain("Shipping Method: Standard");
      expect(result).toContain("Expected Delivery: 7-10 business days");
      const ordersCount = Object.keys(state.db.credit_card_orders.data).length;
      expect(ordersCount).toBe(1);
      const order = Object.values(
        state.db.credit_card_orders.data
      )[0] as Record<string, unknown>;
      expect(order.status).toBe("ORDERED");
      expect(order.reason).toBe("lost");
    });
    it("should close old card on replacement", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "order_replacement_credit_card_7291"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        shipping_address: "123 Main St",
        reason: "stolen",
      });
      const ccAccount = state.db.credit_card_accounts.data.cc_001 as Record<
        string,
        unknown
      >;
      expect(ccAccount.status).toBe("CLOSED");
      expect(ccAccount.closed_date).toBe("11/14/2025");
    });
    it("should reject missing parameters", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "order_replacement_credit_card_7291"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
      });
      expect(result).toContain("Error: Missing required parameters");
    });
    it("should reject invalid reason", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "order_replacement_credit_card_7291"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        shipping_address: "123 Main St",
        reason: "invalid_reason",
      });
      expect(result).toContain("Error: Invalid reason");
    });
    it("should reject nonexistent card account", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "order_replacement_credit_card_7291"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "nonexistent",
        user_id: "user_123",
        shipping_address: "123 Main St",
        reason: "lost",
      });
      expect(result).toContain(
        "Error: Credit card account 'nonexistent' not found"
      );
    });
  });
  describe("get_user_dispute_history_7291", () => {
    it("should return no disputes when none exist", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "get_user_dispute_history_7291"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        user_id: "user_123",
      });
      expect(result).toContain(
        "User transaction dispute history retrieved successfully"
      );
      expect(result).toContain("No transaction disputes found for this user");
    });
    it("should list disputes if they exist", () => {
      state.db.transaction_disputes.data.dispute_001 = {
        dispute_id: "dispute_001",
        user_id: "user_123",
        status: "SUBMITTED",
      };
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "get_user_dispute_history_7291"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        user_id: "user_123",
      });
      expect(result).toContain("1. Dispute ID: dispute_001");
    });
  });
  describe("get_pending_replacement_orders_5765", () => {
    it("should return no orders when none exist", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "get_pending_replacement_orders_5765"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
      });
      expect(result).toContain("Pending replacement orders check completed");
      expect(result).toContain("No pending replacement orders found");
    });
  });
  describe("log_credit_card_closure_reason_4521", () => {
    it("should log closure reason successfully", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "log_credit_card_closure_reason_4521"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        closure_reason: "annual_fee",
      });
      expect(result).toContain("Closure reason logged successfully");
      expect(result).toContain("'annual_fee'");
      const reasonsCount = Object.keys(
        state.db.credit_card_closure_reasons.data
      ).length;
      expect(reasonsCount).toBe(1);
      const reason = Object.values(
        state.db.credit_card_closure_reasons.data
      )[0] as Record<string, unknown>;
      expect(reason.closure_reason).toBe("annual_fee");
      expect(reason.status).toBe("LOGGED");
    });
    it("should reject invalid closure reason", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "log_credit_card_closure_reason_4521"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        closure_reason: "invalid_reason",
      });
      expect(result).toContain("Error: Invalid closure_reason");
    });
  });
  describe("close_credit_card_account_7834", () => {
    it("should close credit card account successfully", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "close_credit_card_account_7834"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
      });
      expect(result).toContain("Credit card account closed successfully");
      expect(result).toContain("cc_001");
      const ccAccount = state.db.credit_card_accounts.data.cc_001 as Record<
        string,
        unknown
      >;
      expect(ccAccount.status).toBe("CLOSED");
      expect(ccAccount.closed_date).toBe("11/14/2025");
      expect(ccAccount.closed_by).toBe("user_123");
    });
    it("should reject nonexistent account", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "close_credit_card_account_7834"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "nonexistent",
        user_id: "user_123",
      });
      expect(result).toContain(
        "Error: Credit card account 'nonexistent' not found"
      );
    });
  });
  describe("pay_credit_card_from_checking_9182", () => {
    it("should process payment successfully", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "pay_credit_card_from_checking_9182"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        user_id: "user_123",
        checking_account_id: "checking_001",
        credit_card_account_id: "cc_001",
        amount: 500,
      });
      expect(result).toContain("Payment processed successfully");
      expect(result).toContain("$500.00");
      expect(result).toContain("$4500.00");
      expect(result).toContain("$1000.00");
      const checkingAccount = state.db.accounts.data.checking_001 as Record<
        string,
        unknown
      >;
      expect(checkingAccount.current_holdings).toBe("4500.00");
      const ccAccount = state.db.credit_card_accounts.data.cc_001 as Record<
        string,
        unknown
      >;
      expect(ccAccount.current_balance).toBe("$1000.00");
    });
    it("should reject a non-numeric amount", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "pay_credit_card_from_checking_9182"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        user_id: "user_123",
        checking_account_id: "checking_001",
        credit_card_account_id: "cc_001",
        amount: "abc",
      });
      expect(result).toBe(
        "Error: Invalid payment amount. Must be a positive number."
      );
    });
    it("should reject insufficient funds", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "pay_credit_card_from_checking_9182"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        user_id: "user_123",
        checking_account_id: "checking_001",
        credit_card_account_id: "cc_001",
        amount: 10000,
      });
      expect(result).toContain("Error: Insufficient funds");
    });
    it("should reject payment exceeding card balance", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "pay_credit_card_from_checking_9182"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        user_id: "user_123",
        checking_account_id: "checking_001",
        credit_card_account_id: "cc_001",
        amount: 2000,
      });
      expect(result).toContain("Error: Payment amount");
      expect(result).toContain("exceeds credit card balance");
    });
    it("should reject negative amount", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "pay_credit_card_from_checking_9182"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        user_id: "user_123",
        checking_account_id: "checking_001",
        credit_card_account_id: "cc_001",
        amount: -100,
      });
      expect(result).toContain(
        "Error: Payment amount must be a positive number"
      );
    });
  });
  describe("submit_credit_limit_increase_request_7392", () => {
    it("should submit CLI request successfully", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "submit_credit_limit_increase_request_7392"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        requested_increase_amount: 2500,
      });
      expect(result).toContain(
        "Credit limit increase request submitted successfully"
      );
      expect(result).toContain("PENDING");
      expect(result).toContain("$2,500");
      const requestsCount = Object.keys(
        state.db.credit_limit_increase_requests.data
      ).length;
      expect(requestsCount).toBe(1);
      const request = Object.values(
        state.db.credit_limit_increase_requests.data
      )[0] as Record<string, unknown>;
      expect(request.status).toBe("PENDING");
      expect(request.requested_increase_amount).toBe(2500);
    });
    it("should reject nonexistent card", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "submit_credit_limit_increase_request_7392"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "nonexistent",
        user_id: "user_123",
        requested_increase_amount: 2500,
      });
      expect(result).toContain(
        "Error: Credit card account 'nonexistent' not found"
      );
    });
    it("should reject zero increase amount", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "submit_credit_limit_increase_request_7392"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        requested_increase_amount: 0,
      });
      expect(result).toContain(
        "Error: Requested increase amount must be positive"
      );
    });
    it("should reject fractional increase amounts without truncating", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "submit_credit_limit_increase_request_7392"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        requested_increase_amount: 2500.5,
      });
      expect(result).toBe(
        "Error: Invalid requested_increase_amount. Must be a whole number of dollars."
      );
      expect(
        Object.keys(state.db.credit_limit_increase_requests.data)
      ).toHaveLength(0);
    });
  });
  describe("get_credit_limit_increase_history_4829", () => {
    it("should return empty history when no requests", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "get_credit_limit_increase_history_4829"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
      });
      expect(result).toContain("Credit limit increase history retrieved");
      expect(result).toContain("No credit limit increase requests found");
    });
  });
  describe("get_payment_history_6183", () => {
    it("should retrieve payment history", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get("get_payment_history_6183");
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        months: 3,
      });
      expect(result).toContain("Payment history for account 'cc_001'");
      expect(result).toContain("Consecutive on-time payments: 2");
      expect(result).toContain("Payment Date:");
    });
    it("should return no history for nonexistent account", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get("get_payment_history_6183");
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "nonexistent",
        months: 3,
      });
      expect(result).toContain("No payment history found");
    });
    it("should reject non-numeric months", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get("get_payment_history_6183");
      if (!tool) {
        throw new Error("Tool not found");
      }
      expect(
        tool.handler(state, {
          credit_card_account_id: "cc_001",
          months: "6.5",
        })
      ).toBe("Error: Invalid months value. Must be a positive integer.");
    });
    it("should truncate numeric float months like Python int()", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get("get_payment_history_6183");
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        months: 1.9,
      });
      expect(result).toContain("(last 1 months):");
      expect(result.match(/Payment Date:/g)).toHaveLength(1);
    });
  });
  describe("approve_credit_limit_increase_5847", () => {
    it("should approve CLI and update limit", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "approve_credit_limit_increase_5847"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        new_credit_limit: 8000,
      });
      expect(result).toContain("Credit limit increase approved!");
      expect(result).toContain("$8000.00");
      const ccAccount = state.db.credit_card_accounts.data.cc_001 as Record<
        string,
        unknown
      >;
      expect(ccAccount.credit_limit).toBe("$8000.00");
    });
    it("should reject if pending disputes exist", () => {
      state.db.transaction_disputes.data.dispute_001 = {
        dispute_id: "dispute_001",
        user_id: "user_123",
        status: "PENDING",
      };
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "approve_credit_limit_increase_5847"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        new_credit_limit: 8000,
      });
      expect(result).toContain(
        "Error: Credit limit increase request cannot be approved"
      );
    });
    it("should reject if account is CLOSED", () => {
      const ccAccount = state.db.credit_card_accounts.data.cc_001 as Record<
        string,
        unknown
      >;
      ccAccount.account_status = "CLOSED";
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "approve_credit_limit_increase_5847"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        new_credit_limit: 8000,
      });
      expect(result).toContain(
        "Error: Credit limit increase request cannot be approved"
      );
    });
  });
  describe("deny_credit_limit_increase_5848", () => {
    it("should deny CLI with reason", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "deny_credit_limit_increase_5848"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        denial_reason: "insufficient_account_age",
      });
      expect(result).toContain("Credit limit increase request denied");
      expect(result).toContain("insufficient_account_age");
      const requestsCount = Object.keys(
        state.db.credit_limit_increase_requests.data
      ).length;
      expect(requestsCount).toBe(1);
      const request = Object.values(
        state.db.credit_limit_increase_requests.data
      )[0] as Record<string, unknown>;
      expect(request.status).toBe("DENIED");
      expect(request.denial_reason).toBe("insufficient_account_age");
    });
    it("should reject invalid denial reason", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "deny_credit_limit_increase_5848"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        denial_reason: "invalid_reason",
      });
      expect(result).toContain("Error: Invalid denial_reason");
    });
  });
  describe("apply_statement_credit_8472", () => {
    it("should apply statement credit successfully", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get("apply_statement_credit_8472");
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        user_id: "user_123",
        credit_card_account_id: "cc_001",
        amount: 50,
        reason: "goodwill_adjustment",
      });
      expect(result).toContain("Statement credit applied successfully");
      expect(result).toContain("$50.00");
      expect(result).toContain("Goodwill Adjustment");
      const historyCount = Object.keys(
        state.db.credit_card_transaction_history.data
      ).length;
      expect(historyCount).toBe(1);
      const credit = Object.values(
        state.db.credit_card_transaction_history.data
      )[0] as Record<string, unknown>;
      expect(credit.credit_reason).toBe("goodwill_adjustment");
      expect(credit.status).toBe("COMPLETED");
    });
    it("should reject zero amount", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get("apply_statement_credit_8472");
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        user_id: "user_123",
        credit_card_account_id: "cc_001",
        amount: 0,
        reason: "goodwill_adjustment",
      });
      expect(result).toContain("Error: Credit amount must be positive");
    });
    it("should reject nonexistent card", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get("apply_statement_credit_8472");
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        user_id: "user_123",
        credit_card_account_id: "nonexistent",
        amount: 50,
        reason: "goodwill_adjustment",
      });
      expect(result).toContain(
        "Error: Credit card account 'nonexistent' not found"
      );
    });
  });
  describe("apply_credit_card_account_flag_6147", () => {
    it("should apply account flag successfully", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_credit_card_account_flag_6147"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        flag_type: "annual_fee_waived",
        expiration_date: "12/31/2025",
        reason: "retention_offer",
      });
      expect(result).toContain("Account flag applied successfully");
      expect(result).toContain("Annual Fee Waived");
      expect(result).toContain("Retention Offer");
      const flagsCount = Object.keys(
        state.db.credit_card_account_flags.data
      ).length;
      expect(flagsCount).toBe(1);
      const flag = Object.values(
        state.db.credit_card_account_flags.data
      )[0] as Record<string, unknown>;
      expect(flag.flag_type).toBe("annual_fee_waived");
      expect(flag.reason).toBe("retention_offer");
      expect(flag.status).toBe("ACTIVE");
    });
    it("should reject invalid flag type", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_credit_card_account_flag_6147"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        flag_type: "invalid_flag",
        expiration_date: "12/31/2025",
        reason: "retention_offer",
      });
      expect(result).toContain("Error: Invalid flag_type");
    });
    it("should reject invalid reason", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "apply_credit_card_account_flag_6147"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
        user_id: "user_123",
        flag_type: "annual_fee_waived",
        expiration_date: "12/31/2025",
        reason: "invalid_reason",
      });
      expect(result).toContain("Error: Invalid reason");
    });
  });
  describe("get_closure_reason_history_8293", () => {
    it("should return empty closure reason history", () => {
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "get_closure_reason_history_8293"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
      });
      expect(result).toContain("Closure reason history retrieved successfully");
      expect(result).toContain("No closure reason records found");
    });
    it("should list closure reasons if they exist", () => {
      state.db.credit_card_closure_reasons.data.reason_001 = {
        record_id: "reason_001",
        credit_card_account_id: "cc_001",
        closure_reason: "annual_fee",
        status: "LOGGED",
      };
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "get_closure_reason_history_8293"
      );
      if (!tool) {
        throw new Error("Tool not found");
      }
      const result = tool.handler(state, {
        credit_card_account_id: "cc_001",
      });
      expect(result).toContain("1. Record ID: reason_001");
    });
  });
});
