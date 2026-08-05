import { expect, test } from "bun:test";
import assert from "node:assert/strict";

import { isRecord } from "../../../internal/guards";
import { makeEmptyBankingData } from "../environment";
import type { BankingData } from "../types";
import { addToDb, queryDb } from "./db-query";
import { registerDisputeTools } from "./handlers-disputes";
import { generateDisputeId } from "./ids";
import type { BankingEnvState } from "./registry";
import { makeBankingEnvState, DISCOVERABLE_AGENT_TOOLS } from "./registry";

function createTestDb(): BankingData {
  return {
    ...makeEmptyBankingData(),
    users: {
      data: {
        user_123: {
          user_id: "user_123",
          name: "John Doe",
          email: "john@example.com",
          phone_number: "555-0123",
          address: "123 Main St",
          date_of_birth: "01/01/1990",
        },
        user_456: {
          user_id: "user_456",
          name: "Jane Smith",
          email: "jane@example.com",
          phone_number: "555-0456",
          address: "456 Oak Ave",
          date_of_birth: "05/15/1985",
        },
      },
    },
    credit_card_transaction_history: {
      data: {
        txn_001: {
          transaction_id: "txn_001",
          user_id: "user_123",
          credit_card_type: "Gold Rewards Card",
          merchant_name: "Amazon",
          transaction_amount: "$99.99",
          transaction_date: "11/10/2025",
          category: "Shopping",
          status: "COMPLETED",
          rewards_earned: "250 points",
        },
        txn_002: {
          transaction_id: "txn_002",
          user_id: "user_456",
          credit_card_type: "Platinum Card",
          merchant_name: "Costco",
          transaction_amount: "$150.00",
          transaction_date: "11/08/2025",
          category: "Groceries",
          status: "COMPLETED",
          rewards_earned: "375 points",
        },
      },
    },
    debit_cards: {
      data: {
        dbc_001: {
          card_id: "dbc_001",
          account_id: "chk_001",
          user_id: "user_123",
          cardholder_name: "JOHN DOE",
          last_4_digits: "5678",
          cvv: "123",
          status: "ACTIVE",
          issue_date: "01/01/2024",
          expiration_date: "12/31/2027",
          issue_reason: "replacement",
          recurring_blocked: false,
        },
      },
    },
    bank_account_transaction_history: {
      data: {
        btxn_deposit_001: {
          transaction_id: "btxn_deposit_001",
          account_id: "chk_001",
          date: "11/10/2025",
          description: "ATM DEPOSIT - RHO-BANK ATM #3921 AUSTIN TX",
          amount: 500,
          type: "atm_deposit",
          status: "posted",
        },
        btxn_834027370c20: {
          transaction_id: "btxn_834027370c20",
          account_id: "chk_001",
          date: "11/10/2025",
          description: "ATM DEPOSIT - RHO-BANK ATM #3921 CEDAR LANE AUSTIN TX",
          amount: 1135,
          type: "atm_deposit",
          status: "posted",
        },
      },
    },
    task_config: {
      data: {
        dispute_settings: {
          auto_resolve_disputes: false,
        },
      },
    },
  };
}
test("registerDisputeTools populates the registry", () => {
  registerDisputeTools();
  expect(DISCOVERABLE_AGENT_TOOLS.has("update_transaction_rewards_3847")).toBe(
    true
  );
  expect(
    DISCOVERABLE_AGENT_TOOLS.has("file_credit_card_transaction_dispute_4829")
  ).toBe(true);
  expect(
    DISCOVERABLE_AGENT_TOOLS.has("file_debit_card_transaction_dispute_6281")
  ).toBe(true);
  expect(
    DISCOVERABLE_AGENT_TOOLS.has("set_debit_card_recurring_block_7382")
  ).toBe(true);
  expect(DISCOVERABLE_AGENT_TOOLS.has("get_debit_dispute_status_7483")).toBe(
    true
  );
  expect(DISCOVERABLE_AGENT_TOOLS.has("get_atm_deposit_images_8473")).toBe(
    true
  );
});
test("update_transaction_rewards_3847: success path", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "update_transaction_rewards_3847"
  )!;
  const result = handler(state, {
    transaction_id: "txn_001",
    new_rewards_earned: "5000 points",
  });
  expect(result).toContain("Transaction rewards updated successfully");
  expect(result).toContain("txn_001");
  expect(result).toContain("5000 points");
  const updated = db.credit_card_transaction_history.data["txn_001"];
  assert(isRecord(updated));
  expect(updated.rewards_earned).toBe("5000 points");
});
test("update_transaction_rewards_3847: validation error - missing transaction_id", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "update_transaction_rewards_3847"
  )!;
  const result = handler(state, { new_rewards_earned: "5000 points" });
  expect(result).toBe("Error: Missing required parameters.");
});
test("update_transaction_rewards_3847: error - transaction not found", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "update_transaction_rewards_3847"
  )!;
  const result = handler(state, {
    transaction_id: "txn_nonexistent",
    new_rewards_earned: "5000 points",
  });
  expect(result).toContain("Error: Transaction 'txn_nonexistent' not found.");
});
test("file_credit_card_transaction_dispute_4829: success with full_refund", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "file_credit_card_transaction_dispute_4829"
  )!;
  const result = handler(state, {
    transaction_id: "txn_001",
    card_action: "keep_active",
    card_last_4_digits: "1234",
    full_name: "John Doe",
    user_id: "user_123",
    phone: "555-0123",
    email: "john@example.com",
    address: "123 Main St",
    contacted_merchant: true,
    purchase_date: "11/10/2025",
    issue_noticed_date: "11/11/2025",
    dispute_reason: "duplicate_charge",
    resolution_requested: "full_refund",
    eligible_for_provisional_credit: true,
  });
  expect(result).toContain(
    "Credit card transaction dispute filed successfully"
  );
  expect(result).toContain("Duplicate Charge");
  expect(result).toContain("Full Refund");
  expect(result).toContain(
    "ELIGIBLE - Credit will be applied within 2 business days"
  );
  const disputeId = generateDisputeId("user_123", "txn_001");
  const disputes = queryDb({
    db,
    dbName: "transaction_disputes",
    constraints: { dispute_id: disputeId },
  });
  expect(disputes.length).toBe(1);
  const dispute = disputes[0];
  assert(isRecord(dispute));
  expect(dispute.dispute_id).toBe(disputeId);
  expect(dispute.transaction_id).toBe("txn_001");
  expect(dispute.user_id).toBe("user_123");
  expect(dispute.status).toBe("SUBMITTED");
  expect(dispute.provisional_credit_given).toBe(true);
});
test("file_credit_card_transaction_dispute_4829: success with partial_refund", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "file_credit_card_transaction_dispute_4829"
  )!;
  const result = handler(state, {
    transaction_id: "txn_002",
    card_action: "cancel_and_reissue",
    card_last_4_digits: "5678",
    full_name: "Jane Smith",
    user_id: "user_456",
    phone: "555-0456",
    email: "jane@example.com",
    address: "456 Oak Ave",
    contacted_merchant: false,
    purchase_date: "11/08/2025",
    issue_noticed_date: "11/09/2025",
    dispute_reason: "incorrect_amount",
    resolution_requested: "partial_refund",
    partial_refund_amount: 50,
    eligible_for_provisional_credit: false,
  });
  expect(result).toContain("Partial Refund Amount: $50.00");
  expect(result).toContain("Not eligible at this time");
  const disputeId = generateDisputeId("user_456", "txn_002");
  const disputes = queryDb({
    db,
    dbName: "transaction_disputes",
    constraints: { dispute_id: disputeId },
  });
  expect(disputes.length).toBe(1);
  const dispute = disputes[0];
  assert(isRecord(dispute));
  expect(dispute.partial_refund_amount).toBe(50);
  expect(dispute.resolution_requested).toBe("partial_refund");
});
test("file_credit_card_transaction_dispute_4829: validation error - invalid dispute_reason", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "file_credit_card_transaction_dispute_4829"
  )!;
  const result = handler(state, {
    transaction_id: "txn_001",
    card_action: "keep_active",
    card_last_4_digits: "1234",
    full_name: "John Doe",
    user_id: "user_123",
    phone: "555-0123",
    email: "john@example.com",
    address: "123 Main St",
    contacted_merchant: true,
    purchase_date: "11/10/2025",
    issue_noticed_date: "11/11/2025",
    dispute_reason: "invalid_reason",
    resolution_requested: "full_refund",
    eligible_for_provisional_credit: false,
  });
  expect(result).toContain("Error: Invalid dispute_reason");
});
test("file_credit_card_transaction_dispute_4829: validation error - partial_refund without amount", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "file_credit_card_transaction_dispute_4829"
  )!;
  const result = handler(state, {
    transaction_id: "txn_001",
    card_action: "keep_active",
    card_last_4_digits: "1234",
    full_name: "John Doe",
    user_id: "user_123",
    phone: "555-0123",
    email: "john@example.com",
    address: "123 Main St",
    contacted_merchant: true,
    purchase_date: "11/10/2025",
    issue_noticed_date: "11/11/2025",
    dispute_reason: "duplicate_charge",
    resolution_requested: "partial_refund",
    eligible_for_provisional_credit: false,
  });
  expect(result).toContain(
    "Error: partial_refund_amount is required when resolution_requested is 'partial_refund'."
  );
});
test("file_debit_card_transaction_dispute_6281: success with fraud category", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "file_debit_card_transaction_dispute_6281"
  )!;
  const result = handler(state, {
    transaction_id: "btxn_001",
    account_id: "chk_001",
    card_id: "dbc_001",
    user_id: "user_123",
    dispute_category: "unauthorized_transaction",
    transaction_date: "11/10/2025",
    discovery_date: "11/11/2025",
    disputed_amount: 750,
    transaction_type: "pin_purchase",
    card_in_possession: true,
    pin_compromised: "no",
    contacted_merchant: false,
    police_report_filed: true,
    written_statement_provided: true,
    provisional_credit_eligible: true,
    customer_max_liability_amount: 0,
    card_action: "freeze_pending_investigation",
  });
  expect(result).toContain("Dispute ID:");
  expect(result).toContain("btxn_001");
  expect(result).toContain("Disputed Amount: $750.00");
  expect(result).toContain("Freeze Pending Investigation");
  expect(result).toContain("ISSUED");
  const disputeId = generateDisputeId("user_123", "btxn_001");
  const disputes = queryDb({
    db,
    dbName: "debit_card_disputes",
    constraints: { dispute_id: disputeId },
  });
  expect(disputes.length).toBe(1);
  const dispute = disputes[0];
  assert(isRecord(dispute));
  expect(dispute.dispute_id).toBe(disputeId);
  expect(dispute.is_fraud_category).toBe(true);
  expect(dispute.provisional_credit_issued).toBe(true);
  expect(dispute.provisional_credit_amount).toBe(750);
  expect(dispute.status).toBe("OPEN");
});
test("file_debit_card_transaction_dispute_6281: validation error - invalid dispute category", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "file_debit_card_transaction_dispute_6281"
  )!;
  const result = handler(state, {
    transaction_id: "btxn_001",
    account_id: "chk_001",
    card_id: "dbc_001",
    user_id: "user_123",
    dispute_category: "invalid_category",
    transaction_date: "11/10/2025",
    discovery_date: "11/11/2025",
    disputed_amount: 750,
    transaction_type: "pin_purchase",
    card_in_possession: true,
    pin_compromised: "no",
    contacted_merchant: false,
    police_report_filed: false,
    written_statement_provided: true,
    provisional_credit_eligible: false,
    customer_max_liability_amount: 0,
    card_action: "keep_active",
  });
  expect(result).toContain("Error: Invalid dispute_category");
});
test("file_debit_card_transaction_dispute_6281: validation error - negative amount", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "file_debit_card_transaction_dispute_6281"
  )!;
  const result = handler(state, {
    transaction_id: "btxn_001",
    account_id: "chk_001",
    card_id: "dbc_001",
    user_id: "user_123",
    dispute_category: "duplicate_charge",
    transaction_date: "11/10/2025",
    discovery_date: "11/11/2025",
    disputed_amount: -100,
    transaction_type: "pin_purchase",
    card_in_possession: true,
    pin_compromised: "no",
    contacted_merchant: false,
    police_report_filed: false,
    written_statement_provided: true,
    provisional_credit_eligible: false,
    customer_max_liability_amount: 0,
    card_action: "keep_active",
  });
  expect(result).toContain("Error: disputed_amount must be a positive number.");
});
test("file_debit_card_transaction_dispute_6281: validation error - missing boolean field", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "file_debit_card_transaction_dispute_6281"
  )!;
  const result = handler(state, {
    transaction_id: "btxn_001",
    account_id: "chk_001",
    card_id: "dbc_001",
    user_id: "user_123",
    dispute_category: "duplicate_charge",
    transaction_date: "11/10/2025",
    discovery_date: "11/11/2025",
    disputed_amount: 100,
    transaction_type: "pin_purchase",
    pin_compromised: "no",
    provisional_credit_eligible: false,
    customer_max_liability_amount: 0,
    card_action: "keep_active",
  });
  expect(result).toContain(
    "Error: card_in_possession, contacted_merchant, police_report_filed, and written_statement_provided are required boolean fields."
  );
});
test("set_debit_card_recurring_block_7382: success - block recurring", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "set_debit_card_recurring_block_7382"
  )!;
  const result = handler(state, {
    card_id: "dbc_001",
    block_recurring: true,
  });
  expect(result).toContain("Recurring payments BLOCKED");
  expect(result).toContain("dbc_001");
  expect(result).toContain(
    "All recurring/subscription charges will be declined"
  );
  const card = db.debit_cards.data["dbc_001"];
  assert(isRecord(card));
  expect(card.recurring_blocked).toBe(true);
});
test("set_debit_card_recurring_block_7382: success - unblock recurring", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "set_debit_card_recurring_block_7382"
  )!;
  const result = handler(state, {
    card_id: "dbc_001",
    block_recurring: false,
  });
  expect(result).toContain("Recurring payments UNBLOCKED");
  expect(result).toContain("will now be allowed");
  const card = db.debit_cards.data["dbc_001"];
  assert(isRecord(card));
  expect(card.recurring_blocked).toBe(false);
});
test("set_debit_card_recurring_block_7382: error - card not found", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "set_debit_card_recurring_block_7382"
  )!;
  const result = handler(state, {
    card_id: "dbc_nonexistent",
    block_recurring: true,
  });
  expect(result).toContain("Error: Debit card 'dbc_nonexistent' not found.");
});
test("set_debit_card_recurring_block_7382: error - card not active", () => {
  registerDisputeTools();
  const db = createTestDb();
  const inactiveCard = db.debit_cards.data["dbc_inactive"];
  if (!inactiveCard) {
    db.debit_cards.data["dbc_inactive"] = {
      card_id: "dbc_inactive",
      account_id: "chk_002",
      user_id: "user_456",
      cardholder_name: "JANE SMITH",
      last_4_digits: "9876",
      status: "SUSPENDED",
      recurring_blocked: false,
    };
  }
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "set_debit_card_recurring_block_7382"
  )!;
  const result = handler(state, {
    card_id: "dbc_inactive",
    block_recurring: true,
  });
  expect(result).toContain("Error: Cannot update recurring block settings");
  expect(result).toContain("Card must be ACTIVE");
});
test("get_debit_dispute_status_7483: success with disputes", () => {
  registerDisputeTools();
  const db = createTestDb();
  const disputeId = generateDisputeId("user_123", "btxn_001");
  addToDb({
    db,
    dbName: "debit_card_disputes",
    recordId: disputeId,
    record: {
      dispute_id: disputeId,
      transaction_id: "btxn_001",
      user_id: "user_123",
      status: "OPEN",
      disputed_amount: 500,
    },
  });
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "get_debit_dispute_status_7483"
  )!;
  const result = handler(state, { user_id: "user_123" });
  expect(result).toContain("Debit card dispute history retrieved successfully");
  expect(result).toContain("user_123");
});
test("get_debit_dispute_status_7483: success - no disputes", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "get_debit_dispute_status_7483"
  )!;
  const result = handler(state, { user_id: "user_456" });
  expect(result).toContain("Debit card dispute history retrieved successfully");
  expect(result).toContain("No debit card disputes found for this user");
});
test("get_atm_deposit_images_8473: success with hardcoded image data", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "get_atm_deposit_images_8473"
  )!;
  const result = handler(state, { transaction_id: "btxn_834027370c20" });
  expect(result).toContain("ATM Deposit Image Retrieval Results");
  expect(result).toContain("btxn_834027370c20");
  expect(result).toContain("ATM DEPOSIT ENVELOPE SCAN");
  expect(result).toContain("$1,135.00");
  expect(result).toContain("Derek Yamamoto");
  expect(result).toContain("$750.00 difference");
});
test("get_atm_deposit_images_8473: success with generic image response", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "get_atm_deposit_images_8473"
  )!;
  const result = handler(state, { transaction_id: "btxn_deposit_001" });
  expect(result).toContain("ATM Deposit Image Retrieval Results");
  expect(result).toContain("btxn_deposit_001");
  expect(result).toContain("IMAGE STATUS");
  expect(result).toContain("IMAGES NOT AVAILABLE");
});
test("get_atm_deposit_images_8473: error - transaction not found", () => {
  registerDisputeTools();
  const db = createTestDb();
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "get_atm_deposit_images_8473"
  )!;
  const result = handler(state, { transaction_id: "btxn_nonexistent" });
  expect(result).toContain("Error: Transaction 'btxn_nonexistent' not found.");
});
test("get_atm_deposit_images_8473: error - not an ATM deposit", () => {
  registerDisputeTools();
  const db = createTestDb();
  db.bank_account_transaction_history.data["btxn_withdrawal"] = {
    transaction_id: "btxn_withdrawal",
    account_id: "chk_001",
    date: "11/10/2025",
    description: "ATM WITHDRAWAL - RHO-BANK ATM #3921 AUSTIN TX",
    amount: -100,
    type: "atm_withdrawal",
    status: "posted",
  };
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "get_atm_deposit_images_8473"
  )!;
  const result = handler(state, { transaction_id: "btxn_withdrawal" });
  expect(result).toContain(
    "Error: Transaction 'btxn_withdrawal' is not an ATM deposit."
  );
});
test("get_atm_deposit_images_8473: error - third-party ATM", () => {
  registerDisputeTools();
  const db = createTestDb();
  db.bank_account_transaction_history.data["btxn_third_party"] = {
    transaction_id: "btxn_third_party",
    account_id: "chk_001",
    date: "11/10/2025",
    description: "ATM DEPOSIT - CHASE BANK ATM #5000 CHICAGO IL",
    amount: 500,
    type: "atm_deposit",
    status: "posted",
  };
  const state: BankingEnvState = makeBankingEnvState(db);
  const { handler } = DISCOVERABLE_AGENT_TOOLS.get(
    "get_atm_deposit_images_8473"
  )!;
  const result = handler(state, { transaction_id: "btxn_third_party" });
  expect(result).toContain("Error: Transaction");
  expect(result).toContain("is from a third-party ATM");
  expect(result).toContain("Rho-Bank ATM");
});
