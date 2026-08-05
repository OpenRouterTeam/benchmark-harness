import { describe, it, expect, beforeEach } from "bun:test";

import { registerDebitTools } from "./handlers-debit";
import type { BankingEnvState } from "./registry";
import { makeBankingEnvState, DISCOVERABLE_AGENT_TOOLS } from "./registry";

const fixtureDebitCards = {
  dbc_test_active: {
    card_id: "dbc_test_active",
    account_id: "chk_test_01",
    user_id: "user_test_01",
    cardholder_name: "JOHN DOE",
    last_4_digits: "5678",
    cvv: "123",
    status: "ACTIVE",
    issue_date: "11/14/2025",
    expiration_date: "11/30/2029",
    card_design: "CLASSIC",
    issue_reason: "new_account",
  },
  dbc_test_pending: {
    card_id: "dbc_test_pending",
    account_id: "chk_test_02",
    user_id: "user_test_02",
    cardholder_name: "JANE SMITH",
    last_4_digits: "9876",
    cvv: "456",
    status: "PENDING",
    issue_date: "11/14/2025",
    expiration_date: "11/30/2029",
    card_design: "PREMIUM",
    issue_reason: "first_card",
  },
  dbc_test_pending_lost: {
    card_id: "dbc_test_pending_lost",
    account_id: "chk_test_03",
    user_id: "user_test_03",
    cardholder_name: "ALICE JOHNSON",
    last_4_digits: "1234",
    cvv: "789",
    status: "PENDING",
    issue_date: "11/14/2025",
    expiration_date: "11/30/2029",
    card_design: "CLASSIC",
    issue_reason: "lost",
  },
  dbc_test_pending_expired: {
    card_id: "dbc_test_pending_expired",
    account_id: "chk_test_04",
    user_id: "user_test_04",
    cardholder_name: "BOB WILLIAMS",
    last_4_digits: "4567",
    cvv: "321",
    status: "PENDING",
    issue_date: "11/14/2025",
    expiration_date: "11/30/2029",
    card_design: "CUSTOM",
    issue_reason: "expired",
  },
};

const fixtureAccounts = {
  chk_test_01: {
    account_id: "chk_test_01",
    user_id: "user_test_01",
    class: "checking",
    level: "Blue Account",
    date_opened: "11/01/2024",
    status: "OPEN",
    current_holdings: "5000.00",
  },
  chk_test_02: {
    account_id: "chk_test_02",
    user_id: "user_test_02",
    class: "checking",
    level: "Green Account",
    date_opened: "09/01/2023",
    status: "OPEN",
    current_holdings: "3200.50",
  },
  chk_test_03: {
    account_id: "chk_test_03",
    user_id: "user_test_03",
    class: "checking",
    level: "Blue Account",
    date_opened: "01/15/2025",
    status: "OPEN",
    current_holdings: "1500.00",
  },
  chk_test_04: {
    account_id: "chk_test_04",
    user_id: "user_test_04",
    class: "checking",
    level: "Green Account",
    date_opened: "03/10/2024",
    status: "OPEN",
    current_holdings: "8500.00",
  },
  chk_test_new_order: {
    account_id: "chk_test_new_order",
    user_id: "user_test_order",
    class: "checking",
    level: "Blue Account",
    date_opened: "06/15/2024",
    status: "OPEN",
    current_holdings: "5500.00",
  },
};

const fixtureUsers = {
  user_test_order: {
    user_id: "user_test_order",
    name: "Test Cardholder",
  },
};

function deepCopyCards(
  cards: Record<string, Record<string, unknown>>
): Record<string, Record<string, unknown>> {
  const result: Record<string, Record<string, unknown>> = {};
  for (const key in cards) {
    result[key] = { ...cards[key] };
  }
  return result;
}

function createTestState(): BankingEnvState {
  const state = makeBankingEnvState({
    users: { data: { ...fixtureUsers } },
    accounts: { data: { ...fixtureAccounts } },
    debit_cards: { data: deepCopyCards(fixtureDebitCards) },
    debit_card_orders: { data: {} },
    bank_account_transaction_history: { data: {} },
    credit_card_transaction_history: { data: {} },
    referrals: { data: {} },
    credit_card_applications: { data: {} },
    user_discoverable_tools: { data: {} },
    user_discoverable_tool_calls: { data: {} },
    verification_history: { data: {} },
    cash_back_disputes: { data: {} },
    credit_card_accounts: { data: {} },
    agent_discoverable_tools: { data: {} },
    task_config: { data: {} },
    human_transfer_requests: { data: {} },
    transaction_disputes: { data: {} },
    credit_card_orders: { data: {} },
    credit_card_closure_reasons: { data: {} },
    credit_card_account_flags: { data: {} },
    credit_limit_increase_requests: { data: {} },
    payment_history: { data: {} },
    debit_card_disputes: { data: {} },
  });
  return state;
}
describe("Debit Card Handlers", () => {
  beforeEach(() => {
    registerDebitTools();
  });
  describe("activate_debit_card_8291", () => {
    it("activates a new card successfully", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("activate_debit_card_8291");
      expect(tool).toBeDefined();
      const result = tool!.handler(state, {
        card_id: "dbc_test_pending",
        last_4_digits: "9876",
        expiration_date: "11/30/2029",
        cvv: "456",
        pin: "2580",
      });
      expect(result).toContain("New Debit Card Activation Successful");
      expect(result).toContain("Status: ACTIVE");
      const card = state.db.debit_cards.data.dbc_test_pending;
      if (!card) {
        throw new Error("Expected dbc_test_pending card");
      }
      expect(card.status).toBe("ACTIVE");
    });
    it("rejects activation with invalid PIN", () => {
      const state = createTestState();
      const badPinCard = {
        ...fixtureDebitCards.dbc_test_pending,
        card_id: "dbc_badpin",
      };
      state.db.debit_cards.data.dbc_badpin = badPinCard;
      const tool = DISCOVERABLE_AGENT_TOOLS.get("activate_debit_card_8291");
      const result = tool!.handler(state, {
        card_id: "dbc_badpin",
        last_4_digits: "9876",
        expiration_date: "11/30/2029",
        cvv: "456",
        pin: "1234",
      });
      expect(result).toContain("Error: PIN cannot be sequential");
      expect(state.db.debit_cards.data.dbc_badpin.status).toBe("PENDING");
    });
    it("rejects activation with wrong issue_reason", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("activate_debit_card_8291");
      const result = tool!.handler(state, {
        card_id: "dbc_test_pending_lost",
        last_4_digits: "1234",
        expiration_date: "11/30/2029",
        cvv: "789",
        pin: "2580",
      });
      expect(result).toContain("Error: Wrong activation tool");
      expect(result).toContain("activate_debit_card_8292");
    });
    it("rejects activation with mismatched last 4 digits", () => {
      const state = createTestState();
      const mismatchCard = {
        ...fixtureDebitCards.dbc_test_pending,
        card_id: "dbc_mismatch",
      };
      state.db.debit_cards.data.dbc_mismatch = mismatchCard;
      const tool = DISCOVERABLE_AGENT_TOOLS.get("activate_debit_card_8291");
      const result = tool!.handler(state, {
        card_id: "dbc_mismatch",
        last_4_digits: "0000",
        expiration_date: "11/30/2029",
        cvv: "456",
        pin: "2580",
      });
      expect(result).toContain("Error: Card verification failed");
    });
  });
  describe("activate_debit_card_8292", () => {
    it("activates a replacement card (lost)", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("activate_debit_card_8292");
      const result = tool!.handler(state, {
        card_id: "dbc_test_pending_lost",
        last_4_digits: "1234",
        expiration_date: "11/30/2029",
        cvv: "789",
        pin: "2580",
      });
      expect(result).toContain("Replacement Debit Card Activation Successful");
      expect(result).toContain("Replacement Reason: Lost");
      expect(result.toUpperCase()).toContain("STATUS: ACTIVE");
    });
    it("activates a replacement card (stolen)", () => {
      const state = createTestState();
      const stolenCard = {
        ...fixtureDebitCards.dbc_test_pending_lost,
        card_id: "dbc_stolen",
        issue_reason: "stolen",
      };
      state.db.debit_cards.data.dbc_stolen = stolenCard;
      const tool = DISCOVERABLE_AGENT_TOOLS.get("activate_debit_card_8292");
      const result = tool!.handler(state, {
        card_id: "dbc_stolen",
        last_4_digits: "1234",
        expiration_date: "11/30/2029",
        cvv: "789",
        pin: "2580",
      });
      expect(result).toContain("Replacement Debit Card Activation Successful");
      expect(state.db.debit_cards.data.dbc_stolen.status).toBe("ACTIVE");
    });
    it("rejects replacement card activation with wrong issue_reason", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("activate_debit_card_8292");
      const result = tool!.handler(state, {
        card_id: "dbc_test_pending",
        last_4_digits: "9876",
        expiration_date: "11/30/2029",
        cvv: "456",
        pin: "2580",
      });
      expect(result).toContain("Error: Wrong activation tool");
      expect(result).toContain("activate_debit_card_8291");
    });
  });
  describe("activate_debit_card_8293", () => {
    it("activates a reissued card (expired)", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("activate_debit_card_8293");
      const result = tool!.handler(state, {
        card_id: "dbc_test_pending_expired",
        last_4_digits: "4567",
        expiration_date: "11/30/2029",
        cvv: "321",
        pin: "2580",
      });
      expect(result).toContain("Reissued Debit Card Activation Successful");
      expect(result).toContain("Reissue Reason: Expired");
      const expiredCard = state.db.debit_cards.data.dbc_test_pending_expired;
      if (!expiredCard) {
        throw new Error("Expected dbc_test_pending_expired");
      }
      expect(expiredCard.status).toBe("ACTIVE");
    });
  });
  describe("freeze_debit_card_3892", () => {
    it("freezes an active card", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("freeze_debit_card_3892");
      const result = tool!.handler(state, {
        card_id: "dbc_test_active",
      });
      expect(result).toContain("Debit Card Frozen Successfully");
      const activeCard1 = state.db.debit_cards.data.dbc_test_active;
      if (!activeCard1) {
        throw new Error("Expected dbc_test_active");
      }
      expect(activeCard1.status).toBe("FROZEN");
    });
    it("rejects freezing a frozen card", () => {
      const state = createTestState();
      const activeCard2 = state.db.debit_cards.data.dbc_test_active;
      if (!activeCard2) {
        throw new Error("Expected dbc_test_active");
      }
      activeCard2.status = "FROZEN";
      const tool = DISCOVERABLE_AGENT_TOOLS.get("freeze_debit_card_3892");
      const result = tool!.handler(state, {
        card_id: "dbc_test_active",
      });
      expect(result).toContain(
        "Error: Debit card 'dbc_test_active' is already frozen"
      );
    });
  });
  describe("unfreeze_debit_card_3893", () => {
    it("unfreezes a frozen card", () => {
      const state = createTestState();
      const activeCard3 = state.db.debit_cards.data.dbc_test_active;
      if (!activeCard3) {
        throw new Error("Expected dbc_test_active");
      }
      activeCard3.status = "FROZEN";
      const tool = DISCOVERABLE_AGENT_TOOLS.get("unfreeze_debit_card_3893");
      const result = tool!.handler(state, {
        card_id: "dbc_test_active",
      });
      expect(result).toContain("Debit Card Unfrozen Successfully");
      expect(activeCard3.status).toBe("ACTIVE");
    });
    it("freeze then unfreeze roundtrip", () => {
      const state = createTestState();
      const freezeTool = DISCOVERABLE_AGENT_TOOLS.get("freeze_debit_card_3892");
      const unfreezeTool = DISCOVERABLE_AGENT_TOOLS.get(
        "unfreeze_debit_card_3893"
      );
      freezeTool!.handler(state, { card_id: "dbc_test_active" });
      const activeCard4 = state.db.debit_cards.data.dbc_test_active;
      if (!activeCard4) {
        throw new Error("Expected dbc_test_active");
      }
      expect(activeCard4.status).toBe("FROZEN");
      const unfreezeResult = unfreezeTool!.handler(state, {
        card_id: "dbc_test_active",
      });
      expect(unfreezeResult).toContain("Debit Card Unfrozen Successfully");
      expect(activeCard4.status).toBe("ACTIVE");
    });
  });
  describe("close_debit_card_4721", () => {
    it("closes an active card", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("close_debit_card_4721");
      const result = tool!.handler(state, {
        card_id: "dbc_test_active",
        reason: "no_longer_needed",
      });
      expect(result).toContain("Debit Card Closed Successfully");
      expect(result).toContain("Closure Reason: No longer needed");
      const activeCard5 = state.db.debit_cards.data.dbc_test_active;
      if (!activeCard5) {
        throw new Error("Expected dbc_test_active");
      }
      expect(activeCard5.status).toBe("CLOSED");
    });
    it("rejects closing a non-existent card", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("close_debit_card_4721");
      const result = tool!.handler(state, {
        card_id: "dbc_nonexistent",
        reason: "lost",
      });
      expect(result).toContain("Error: Debit card 'dbc_nonexistent' not found");
    });
  });
  describe("clear_debit_card_fraud_alert_4892", () => {
    it("clears velocity block", () => {
      const state = createTestState();
      const activeCard6 = state.db.debit_cards.data.dbc_test_active;
      if (!activeCard6) {
        throw new Error("Expected dbc_test_active");
      }
      activeCard6.velocity_blocked = true;
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "clear_debit_card_fraud_alert_4892"
      );
      const result = tool!.handler(state, {
        card_id: "dbc_test_active",
        reason: "velocity_clear",
      });
      expect(result).toContain("Velocity Block Cleared Successfully");
      expect(activeCard6.velocity_blocked).toBe(false);
    });
    it("clears fraud alert", () => {
      const state = createTestState();
      const activeCard7 = state.db.debit_cards.data.dbc_test_active;
      if (!activeCard7) {
        throw new Error("Expected dbc_test_active");
      }
      activeCard7.fraud_alert_active = true;
      activeCard7.alert_source = "customer_reported";
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "clear_debit_card_fraud_alert_4892"
      );
      const result = tool!.handler(state, {
        card_id: "dbc_test_active",
        reason: "customer_verified",
      });
      expect(result).toContain("Fraud Alert Cleared Successfully");
      expect(activeCard7.fraud_alert_active).toBe(false);
    });
  });
  describe("reset_debit_card_pin_6284", () => {
    it("resets PIN on active card", () => {
      const state = createTestState();
      const pinResetCard = {
        ...fixtureDebitCards.dbc_test_active,
        card_id: "dbc_pinreset",
      };
      state.db.debit_cards.data.dbc_pinreset = pinResetCard;
      const tool = DISCOVERABLE_AGENT_TOOLS.get("reset_debit_card_pin_6284");
      const result = tool!.handler(state, {
        card_id: "dbc_pinreset",
        last_4_digits: "5678",
        new_pin: "3692",
      });
      expect(result).toContain("Debit Card PIN Reset Successfully");
      expect(state.db.debit_cards.data.dbc_pinreset.pin_locked).toBe(false);
    });
    it("rejects PIN reset with invalid PIN", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("reset_debit_card_pin_6284");
      const result = tool!.handler(state, {
        card_id: "dbc_test_active",
        last_4_digits: "5678",
        new_pin: "1111",
      });
      expect(result).toContain("Error: PIN cannot be all the same digit");
    });
  });
  describe("change_debit_card_pin_6285", () => {
    it("changes PIN on active card", () => {
      const state = createTestState();
      const pinChangeCard = {
        ...fixtureDebitCards.dbc_test_active,
        card_id: "dbc_pinchange",
      };
      state.db.debit_cards.data.dbc_pinchange = pinChangeCard;
      const tool = DISCOVERABLE_AGENT_TOOLS.get("change_debit_card_pin_6285");
      const result = tool!.handler(state, {
        card_id: "dbc_pinchange",
        current_pin: "3692",
        new_pin: "2580",
      });
      expect(result).toContain("Debit Card PIN Changed Successfully");
    });
    it("rejects PIN change when new PIN equals current PIN", () => {
      const state = createTestState();
      const samePinCard = {
        ...fixtureDebitCards.dbc_test_active,
        card_id: "dbc_samepin",
      };
      state.db.debit_cards.data.dbc_samepin = samePinCard;
      const tool = DISCOVERABLE_AGENT_TOOLS.get("change_debit_card_pin_6285");
      const result = tool!.handler(state, {
        card_id: "dbc_samepin",
        current_pin: "3692",
        new_pin: "3692",
      });
      expect(result).toContain(
        "Error: New PIN must be different from current PIN"
      );
    });
  });
  describe("get_debit_cards_by_account_id_7823", () => {
    it("retrieves debit cards for an account", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "get_debit_cards_by_account_id_7823"
      );
      const result = tool!.handler(state, {
        account_id: "chk_test_01",
      });
      expect(result).not.toContain("Error");
      const parsed = JSON.parse(result);
      expect(Array.isArray(parsed)).toBe(true);
      expect(parsed.length).toBeGreaterThan(0);
      expect(parsed[0].card_id).toBe("dbc_test_active");
    });
    it("returns error for non-existent account", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "get_debit_cards_by_account_id_7823"
      );
      const result = tool!.handler(state, {
        account_id: "chk_nonexistent",
      });
      expect(result).toContain("Error: Account 'chk_nonexistent' not found");
    });
  });
  describe("order_debit_card_5739", () => {
    it("orders a debit card with fees", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("order_debit_card_5739");
      const result = tool!.handler(state, {
        account_id: "chk_test_new_order",
        user_id: "user_test_order",
        delivery_option: "STANDARD",
        delivery_fee: 15,
        card_design: "CLASSIC",
        design_fee: 5,
        shipping_address: "123 Main St, Anytown, USA",
        excess_replacement_fee: 0,
      });
      expect(result).toContain("Debit Card Order Confirmed");
      expect(result).toContain("Total Fees: $20.00");
      expect(result).toContain("7-10 business days");
      const ordersKeys = Object.keys(state.db.debit_card_orders.data);
      expect(ordersKeys.length).toBeGreaterThan(0);
      const orderId = ordersKeys[0];
      if (!orderId) {
        throw new Error("Expected order ID");
      }
      const order = state.db.debit_card_orders.data[orderId];
      if (!order) {
        throw new Error("Expected order");
      }
      expect(order.total_fee).toBe(20);
      expect(order.status).toBe("PENDING");
    });
    it("deducts fees from account balance", () => {
      const state = createTestState();
      const account1 = state.db.accounts.data.chk_test_new_order;
      if (!account1) {
        throw new Error("Expected chk_test_new_order account");
      }
      const initialBalance = Number.parseFloat(
        String(account1.current_holdings)
      );
      const tool = DISCOVERABLE_AGENT_TOOLS.get("order_debit_card_5739");
      tool!.handler(state, {
        account_id: "chk_test_new_order",
        user_id: "user_test_order",
        delivery_option: "EXPEDITED",
        delivery_fee: 25,
        card_design: "PREMIUM",
        design_fee: 10,
        shipping_address: "456 Oak Ave, Somewhere, USA",
        excess_replacement_fee: 5,
      });
      const newBalance = Number.parseFloat(String(account1.current_holdings));
      expect(newBalance).toBe(initialBalance - 40);
    });
    it("creates fee transaction record", () => {
      const state = createTestState();
      const tool = DISCOVERABLE_AGENT_TOOLS.get("order_debit_card_5739");
      tool!.handler(state, {
        account_id: "chk_test_new_order",
        user_id: "user_test_order",
        delivery_option: "RUSH",
        delivery_fee: 50,
        card_design: "CUSTOM",
        design_fee: 15,
        shipping_address: "789 Pine Rd, Elsewhere, USA",
        excess_replacement_fee: 10,
      });
      const txnKeys = Object.keys(
        state.db.bank_account_transaction_history.data
      );
      expect(txnKeys.length).toBeGreaterThan(0);
      const txnId = txnKeys[0];
      if (!txnId) {
        throw new Error("Expected transaction ID");
      }
      const txn = state.db.bank_account_transaction_history.data[txnId];
      if (!txn) {
        throw new Error("Expected transaction");
      }
      expect(txn.type).toBe("debit_card_fee");
      expect(txn.status).toBe("posted");
      expect(txn.amount).toBe(-75);
      expect(String(txn.description)).toContain("DEBIT CARD ORDER FEE");
    });
    it("rejects order with insufficient balance for fees", () => {
      const state = createTestState();
      const account2 = state.db.accounts.data.chk_test_new_order;
      if (!account2) {
        throw new Error("Expected chk_test_new_order account");
      }
      account2.current_holdings = "30.00";
      const tool = DISCOVERABLE_AGENT_TOOLS.get("order_debit_card_5739");
      const result = tool!.handler(state, {
        account_id: "chk_test_new_order",
        user_id: "user_test_order",
        delivery_option: "STANDARD",
        delivery_fee: 15,
        card_design: "CLASSIC",
        design_fee: 5,
        shipping_address: "123 Main St",
        excess_replacement_fee: 20,
      });
      expect(result).toContain("Error: Insufficient funds for fees");
    });
    it("rejects order with insufficient minimum balance", () => {
      const state = createTestState();
      const account3 = state.db.accounts.data.chk_test_new_order;
      if (!account3) {
        throw new Error("Expected chk_test_new_order account");
      }
      account3.current_holdings = "10.00";
      const tool = DISCOVERABLE_AGENT_TOOLS.get("order_debit_card_5739");
      const result = tool!.handler(state, {
        account_id: "chk_test_new_order",
        user_id: "user_test_order",
        delivery_option: "STANDARD",
        delivery_fee: 0,
        card_design: "CLASSIC",
        design_fee: 0,
        shipping_address: "123 Main St",
        excess_replacement_fee: 0,
      });
      expect(result).toContain(
        "Error: Account must have a minimum balance of $25"
      );
    });
  });
  describe("request_temporary_debit_card_limit_increase_8374", () => {
    it("grants ATM limit increase", () => {
      const state = createTestState();
      const limitCard = {
        ...fixtureDebitCards.dbc_test_active,
        card_id: "dbc_lim1",
        daily_atm_limit: 500,
      };
      state.db.debit_cards.data.dbc_lim1 = limitCard;
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "request_temporary_debit_card_limit_increase_8374"
      );
      const result = tool!.handler(state, {
        card_id: "dbc_lim1",
        limit_type: "atm",
        new_limit: 600,
      });
      expect(result).toContain(
        "Temporary Daily ATM Withdrawal Limit Increase Granted Successfully"
      );
      expect(state.db.debit_cards.data.dbc_lim1.daily_atm_limit).toBe(600);
    });
    it("rejects limit increase exceeding 150% of current", () => {
      const state = createTestState();
      const limitCard = {
        ...fixtureDebitCards.dbc_test_active,
        card_id: "dbc_lim2",
        daily_atm_limit: 500,
      };
      state.db.debit_cards.data.dbc_lim2 = limitCard;
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "request_temporary_debit_card_limit_increase_8374"
      );
      const result = tool!.handler(state, {
        card_id: "dbc_lim2",
        limit_type: "atm",
        new_limit: 900,
      });
      expect(result).toContain(
        "Error: Requested limit $900 exceeds the maximum allowed"
      );
    });
    it("rejects limit increase not higher than current", () => {
      const state = createTestState();
      const limitCard = {
        ...fixtureDebitCards.dbc_test_active,
        card_id: "dbc_lim3",
        daily_atm_limit: 500,
      };
      state.db.debit_cards.data.dbc_lim3 = limitCard;
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "request_temporary_debit_card_limit_increase_8374"
      );
      const result = tool!.handler(state, {
        card_id: "dbc_lim3",
        limit_type: "atm",
        new_limit: 500,
      });
      expect(result).toContain(
        "Error: Requested limit $500 is not higher than the current limit"
      );
    });
    it("rejects fractional limits without truncating", () => {
      const state = createTestState();
      const limitCard = {
        ...fixtureDebitCards.dbc_test_active,
        card_id: "dbc_fractional",
        daily_atm_limit: 500,
      };
      state.db.debit_cards.data.dbc_fractional = limitCard;
      const tool = DISCOVERABLE_AGENT_TOOLS.get(
        "request_temporary_debit_card_limit_increase_8374"
      );
      const result = tool!.handler(state, {
        card_id: "dbc_fractional",
        limit_type: "atm",
        new_limit: 600.5,
      });
      expect(result).toBe("Error: new_limit must be an integer, got '600.5'.");
      expect(state.db.debit_cards.data.dbc_fractional.daily_atm_limit).toBe(
        500
      );
    });
  });
});
