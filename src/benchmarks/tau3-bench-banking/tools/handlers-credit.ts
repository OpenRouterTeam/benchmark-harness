import { queryDb, addToDb, updateRecordInDb } from "./db-query";
import { getFieldAsRecord } from "./field-access";
import { registerCreditLimitAndPaymentTools } from "./handlers-credit-2";
import { getTodayStr } from "./helpers";
import {
  generateCreditCardOrderId,
  generateClosureReasonId,
  generateAccountFlagId,
  generateTransactionId,
} from "./ids";
import type {
  BankingEnvState,
  DiscoverableTool,
  ToolParameter,
} from "./registry";
import { registerDiscoverableAgentTool } from "./registry";

function titleCase(str: string): string {
  return str
    .split("_")
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(" ");
}

function orderReplacementCreditCard(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  const userId = String(kwargs.user_id ?? "");
  const shippingAddress = String(kwargs.shipping_address ?? "");
  const reason = String(kwargs.reason ?? "");
  const expeditedShipping = kwargs.expedited_shipping === true;
  if (!creditCardAccountId || !userId || !shippingAddress || !reason) {
    return "Error: Missing required parameters (credit_card_account_id, user_id, shipping_address, reason).";
  }
  const validReasons = [
    "fraud_suspected",
    "lost",
    "stolen",
    "damaged",
    "expired",
    "other",
  ];
  if (!validReasons.includes(reason)) {
    return `Error: Invalid reason. Must be one of: ${validReasons}`;
  }
  const ccAccounts = queryDb({
    db: state.db,
    dbName: "credit_card_accounts",
    constraints: {
      account_id: creditCardAccountId,
    },
  });
  if (ccAccounts.length === 0) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  const orderId = generateCreditCardOrderId(
    creditCardAccountId,
    userId,
    reason
  );
  const today = getTodayStr();
  const orderRecord = {
    order_id: orderId,
    credit_card_account_id: creditCardAccountId,
    user_id: userId,
    shipping_address: shippingAddress,
    reason,
    expedited_shipping: expeditedShipping,
    order_date: today,
    status: "ORDERED",
    old_card_cancelled: true,
  };
  const success = addToDb({
    db: state.db,
    dbName: "credit_card_orders",
    recordId: orderId,
    record: orderRecord,
  });
  if (!success) {
    return "Error: Order may have already been placed for this card replacement.";
  }
  const ccTable = state.db.credit_card_accounts;
  if (creditCardAccountId in ccTable.data) {
    const record = getFieldAsRecord(ccTable.data, creditCardAccountId);
    if (record) {
      record.status = "CLOSED";
      record.closed_date = today;
    }
  }
  const shippingMethod = expeditedShipping ? "Expedited" : "Standard";
  const expectedDelivery = expeditedShipping
    ? "2-3 business days"
    : "7-10 business days";
  const resultParts = [
    `Order ID: ${orderId}`,
    `Card Account: ${creditCardAccountId}`,
    `Reason: ${titleCase(reason)}`,
    `Shipping Address: ${shippingAddress}`,
    `Shipping Method: ${shippingMethod}`,
    `Expected Delivery: ${expectedDelivery}`,
    "",
    "The old card has been cancelled for security. The new card will have the same account number but a new card number and CVV.",
  ];
  return resultParts.join("\n");
}

const orderReplacementCreditCardTool: DiscoverableTool = {
  name: "order_replacement_credit_card_7291",
  description:
    "Order a replacement credit card for a customer. The old card will be automatically cancelled when the replacement is ordered.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The credit card account ID for the card being replaced",
    },
    {
      name: "user_id",
      type: "string",
      optional: false,
      description: "The user's unique identifier in the system",
    },
    {
      name: "shipping_address",
      type: "string",
      optional: false,
      description: "Full shipping address where the new card should be sent",
    },
    {
      name: "reason",
      type: "string",
      optional: false,
      description:
        "Reason for replacement. Must be one of: 'fraud_suspected', 'lost', 'stolen', 'damaged', 'expired', 'other'",
    },
    {
      name: "expedited_shipping",
      type: "boolean",
      optional: true,
      description:
        "Whether to use expedited shipping (2-3 business days instead of 7-10)",
    },
  ] satisfies readonly ToolParameter[],
  handler: orderReplacementCreditCard,
};

function getUserDisputeHistory(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = String(kwargs.user_id ?? "");
  if (!userId) {
    return "Error: Missing required parameter: user_id";
  }
  const disputes = queryDb({
    db: state.db,
    dbName: "transaction_disputes",
    constraints: { user_id: userId },
  });
  const resultParts = [
    "User transaction dispute history retrieved successfully.",
    "",
    "Executed: get_user_dispute_history_7291",
    `Transaction dispute history for user ${userId}:`,
  ];
  if (disputes.length === 0) {
    resultParts.push("\nNo transaction disputes found for this user.");
  } else {
    resultParts.push("");
    for (let i = 0; i < disputes.length; i++) {
      const record = disputes[i] as Record<string, unknown>;
      resultParts.push(`${i + 1}. Dispute ID: ${record.dispute_id}`);
      for (const [field, value] of Object.entries(record)) {
        resultParts.push(`   ${field}: ${value}`);
      }
      resultParts.push("");
    }
  }
  return resultParts.join("\n");
}

const getUserDisputeHistoryTool: DiscoverableTool = {
  name: "get_user_dispute_history_7291",
  mutatesState: false,
  description:
    "Retrieve a user's credit card transaction dispute history from the transaction_disputes table. Returns all credit card transaction disputes filed by the user, including dispute IDs, transaction IDs, dispute reasons, statuses, and submission dates.",
  params: [
    {
      name: "user_id",
      type: "string",
      optional: false,
      description: "The user's unique identifier in the system",
    },
  ] satisfies readonly ToolParameter[],
  handler: getUserDisputeHistory,
};

function getPendingReplacementOrders(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  if (!creditCardAccountId) {
    return "Error: Missing required parameter: credit_card_account_id";
  }
  const orders = queryDb({
    db: state.db,
    dbName: "credit_card_orders",
    constraints: {
      credit_card_account_id: creditCardAccountId,
    },
  });
  const resultParts = [
    "Pending replacement orders check completed.",
    "",
    "Executed: get_pending_replacement_orders_5765",
    `Replacement orders for credit card account ${creditCardAccountId}:`,
  ];
  if (orders.length === 0) {
    resultParts.push(
      "\nNo pending replacement orders found for this credit card account."
    );
  } else {
    resultParts.push("");
    for (let i = 0; i < orders.length; i++) {
      const record = orders[i] as Record<string, unknown>;
      resultParts.push(`${i + 1}. Order ID: ${record.order_id}`);
      for (const [field, value] of Object.entries(record)) {
        resultParts.push(`   ${field}: ${value}`);
      }
      resultParts.push("");
    }
  }
  return resultParts.join("\n");
}

const getPendingReplacementOrdersTool: DiscoverableTool = {
  name: "get_pending_replacement_orders_5765",
  mutatesState: false,
  description:
    "Check if a credit card account has any pending replacement card orders.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description:
        "The credit card account ID to check for pending replacement orders",
    },
  ] satisfies readonly ToolParameter[],
  handler: getPendingReplacementOrders,
};

function logCreditCardClosureReason(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  const userId = String(kwargs.user_id ?? "");
  const closureReason = String(kwargs.closure_reason ?? "");
  if (!creditCardAccountId || !userId || !closureReason) {
    return "Error: Missing required parameters.";
  }
  const validReasons = [
    "annual_fee",
    "not_using_card",
    "found_better_card",
    "unhappy_with_rewards",
    "simplifying_finances",
    "negative_experience",
    "other",
  ];
  if (!validReasons.includes(closureReason)) {
    return `Error: Invalid closure_reason. Must be one of: ${validReasons}`;
  }
  const recordId = generateClosureReasonId(creditCardAccountId, userId);
  const closureRecord = {
    record_id: recordId,
    credit_card_account_id: creditCardAccountId,
    user_id: userId,
    closure_reason: closureReason,
    logged_at: getTodayStr(),
    status: "LOGGED",
  };
  addToDb({
    db: state.db,
    dbName: "credit_card_closure_reasons",
    recordId,
    record: closureRecord,
  });
  return (
    `Closure reason logged successfully.\n\n` +
    `Executed: log_credit_card_closure_reason_4521\n` +
    `Arguments: ${JSON.stringify({ credit_card_account_id: creditCardAccountId, user_id: userId, closure_reason: closureReason }, null, 2)}\n` +
    `Closure reason '${closureReason}' logged for account ${creditCardAccountId}.`
  );
}

const logCreditCardClosureReasonTool: DiscoverableTool = {
  name: "log_credit_card_closure_reason_4521",
  description:
    "Log the reason why a customer wants to close their credit card account.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The credit card account ID the customer wants to close",
    },
    {
      name: "user_id",
      type: "string",
      optional: false,
      description: "The user's unique identifier in the system",
    },
    {
      name: "closure_reason",
      type: "string",
      optional: false,
      description:
        "Reason for closure. Must be one of: 'annual_fee', 'not_using_card', 'found_better_card', 'unhappy_with_rewards', 'simplifying_finances', 'negative_experience', 'other'",
    },
  ] satisfies readonly ToolParameter[],
  handler: logCreditCardClosureReason,
};

function getClosureReasonHistory(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  if (!creditCardAccountId) {
    return "Error: Missing required parameter: credit_card_account_id";
  }
  const closureReasons = queryDb({
    db: state.db,
    dbName: "credit_card_closure_reasons",
    constraints: {
      credit_card_account_id: creditCardAccountId,
    },
  });
  const resultParts = [
    "Closure reason history retrieved successfully.",
    "",
    "Executed: get_closure_reason_history_8293",
    `Closure reason history for credit card account ${creditCardAccountId}:`,
  ];
  if (closureReasons.length === 0) {
    resultParts.push(
      "\nNo closure reason records found for this credit card account."
    );
  } else {
    resultParts.push("");
    for (let i = 0; i < closureReasons.length; i++) {
      const record = closureReasons[i] as Record<string, unknown>;
      resultParts.push(`${i + 1}. Record ID: ${record.record_id}`);
      for (const [field, value] of Object.entries(record)) {
        resultParts.push(`   ${field}: ${value}`);
      }
      resultParts.push("");
    }
  }
  return resultParts.join("\n");
}

const getClosureReasonHistoryTool: DiscoverableTool = {
  name: "get_closure_reason_history_8293",
  mutatesState: false,
  description:
    "Retrieve the closure reason history for a specific credit card account.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description:
        "The credit card account ID to check for previous closure attempts",
    },
  ] satisfies readonly ToolParameter[],
  handler: getClosureReasonHistory,
};

function applyStatementCredit(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = String(kwargs.user_id ?? "");
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  const amountRaw = kwargs.amount;
  const reason = String(kwargs.reason ?? "");
  if (
    !userId ||
    !creditCardAccountId ||
    amountRaw === null ||
    amountRaw === undefined ||
    !reason
  ) {
    return "Error: Missing required parameters (user_id, credit_card_account_id, amount, reason).";
  }
  const amount =
    typeof amountRaw === "number"
      ? amountRaw
      : Number.parseFloat(String(amountRaw));
  if (Number.isNaN(amount)) {
    return "Error: Invalid amount.";
  }
  if (amount <= 0) {
    return "Error: Credit amount must be positive.";
  }
  const validReasons = [
    "goodwill_adjustment",
    "promotional_credit",
    "annual_fee_reversal",
    "late_fee_reversal",
    "interest_charge_reversal",
    "dispute_resolution",
    "price_match",
    "retention_offer",
    "error_correction",
    "other",
  ];
  if (!validReasons.includes(reason)) {
    return `Error: Invalid reason. Must be one of: ${validReasons}`;
  }
  const ccAccounts = queryDb({
    db: state.db,
    dbName: "credit_card_accounts",
    constraints: {
      account_id: creditCardAccountId,
    },
  });
  if (ccAccounts.length === 0) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  const transactionId = generateTransactionId({
    userId,
    creditCardType: "STATEMENT_CREDIT",
    merchantName: reason,
    amount,
    category: "Statement Credit",
  });
  const today = getTodayStr();
  const creditRecord = {
    transaction_id: transactionId,
    user_id: userId,
    credit_card_account_id: creditCardAccountId,
    credit_card_type: "N/A",
    merchant_name: "Rho-Bank Statement Credit",
    transaction_amount: `-$${amount.toFixed(2)}`,
    transaction_date: today,
    category: "Statement Credit",
    status: "COMPLETED",
    rewards_earned: "0 points",
    credit_reason: reason,
  };
  const success = addToDb({
    db: state.db,
    dbName: "credit_card_transaction_history",
    recordId: transactionId,
    record: creditRecord,
  });
  if (!success) {
    return `Error: Failed to apply statement credit. Transaction ID '${transactionId}' may already exist.`;
  }
  return (
    `Statement credit applied successfully.\n\n` +
    `Executed: apply_statement_credit_8472\n` +
    `  - Transaction ID: ${transactionId}\n` +
    `  - User ID: ${userId}\n` +
    `  - Account: ${creditCardAccountId}\n` +
    `  - Credit Amount: $${amount.toFixed(2)}\n` +
    `  - Reason: ${titleCase(reason)}\n` +
    `  - Date: ${today}`
  );
}

const applyStatementCreditTool: DiscoverableTool = {
  name: "apply_statement_credit_8472",
  description: "Apply a statement credit to a customer's credit card account.",
  params: [
    {
      name: "user_id",
      type: "string",
      optional: false,
      description: "The user's unique identifier in the system",
    },
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The credit card account ID to apply the credit to",
    },
    {
      name: "amount",
      type: "number",
      optional: false,
      description:
        "The credit amount in dollars (e.g., 25.00 for a $25 credit)",
    },
    {
      name: "reason",
      type: "string",
      optional: false,
      description:
        "Reason for the statement credit. Must be one of: 'goodwill_adjustment', 'promotional_credit', 'annual_fee_reversal', 'late_fee_reversal', 'interest_charge_reversal', 'dispute_resolution', 'price_match', 'retention_offer', 'error_correction', 'other'",
    },
  ] satisfies readonly ToolParameter[],
  handler: applyStatementCredit,
};

function applyCreditCardAccountFlag(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  const userId = String(kwargs.user_id ?? "");
  const flagType = String(kwargs.flag_type ?? "");
  const expirationDate = String(kwargs.expiration_date ?? "");
  const reason = String(kwargs.reason ?? "");
  if (
    !creditCardAccountId ||
    !userId ||
    !flagType ||
    !expirationDate ||
    !reason
  ) {
    return "Error: Missing required parameters (credit_card_account_id, user_id, flag_type, expiration_date, reason).";
  }
  const validFlagTypes = [
    "annual_fee_waived",
    "promotional_apr",
    "rewards_bonus",
    "other",
  ];
  if (!validFlagTypes.includes(flagType)) {
    return `Error: Invalid flag_type. Must be one of: ${validFlagTypes}`;
  }
  const validReasons = [
    "retention_offer",
    "loyalty_benefit",
    "promotional",
    "error_correction",
    "other",
  ];
  if (!validReasons.includes(reason)) {
    return `Error: Invalid reason. Must be one of: ${validReasons}`;
  }
  const ccAccounts = queryDb({
    db: state.db,
    dbName: "credit_card_accounts",
    constraints: {
      account_id: creditCardAccountId,
    },
  });
  if (ccAccounts.length === 0) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  const flagId = generateAccountFlagId(
    creditCardAccountId,
    flagType,
    expirationDate
  );
  const today = getTodayStr();
  const flagRecord = {
    flag_id: flagId,
    credit_card_account_id: creditCardAccountId,
    user_id: userId,
    flag_type: flagType,
    effective_date: today,
    expiration_date: expirationDate,
    reason,
    applied_at: today,
    status: "ACTIVE",
  };
  const success = addToDb({
    db: state.db,
    dbName: "credit_card_account_flags",
    recordId: flagId,
    record: flagRecord,
  });
  if (!success) {
    return `Error: Failed to apply account flag. Flag ID '${flagId}' may already exist.`;
  }
  const flagTypeDisplay = titleCase(flagType);
  const reasonDisplay = titleCase(reason);
  return (
    `Account flag applied successfully!\n` +
    `  - Flag ID: ${flagId}\n` +
    `  - Account: ${creditCardAccountId}\n` +
    `  - User ID: ${userId}\n` +
    `  - Flag Type: ${flagTypeDisplay}\n` +
    `  - Effective Date: ${today}\n` +
    `  - Expiration Date: ${expirationDate}\n` +
    `  - Reason: ${reasonDisplay}`
  );
}

const applyCreditCardAccountFlagTool: DiscoverableTool = {
  name: "apply_credit_card_account_flag_6147",
  description:
    "Apply a flag to a customer's credit card account. Flags can include annual fee waivers, promotional APR rates, rewards bonuses, or other account-level modifiers. Each flag has an effective date and expiration date.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The credit card account ID to apply the flag to",
    },
    {
      name: "user_id",
      type: "string",
      optional: false,
      description: "The user's unique identifier in the system",
    },
    {
      name: "flag_type",
      type: "string",
      optional: false,
      description:
        "Type of flag to apply. Must be one of: 'annual_fee_waived', 'promotional_apr', 'rewards_bonus', 'other'",
    },
    {
      name: "expiration_date",
      type: "string",
      optional: false,
      description: "Date when the flag expires (MM/DD/YYYY format)",
    },
    {
      name: "reason",
      type: "string",
      optional: false,
      description:
        "Reason for applying this flag. Must be one of: 'retention_offer', 'loyalty_benefit', 'promotional', 'error_correction', 'other'",
    },
  ] satisfies readonly ToolParameter[],
  handler: applyCreditCardAccountFlag,
};

function closeCreditCardAccount(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  const userId = String(kwargs.user_id ?? "");
  if (!creditCardAccountId || !userId) {
    return "Error: Missing required parameters (credit_card_account_id, user_id).";
  }
  const ccAccounts = queryDb({
    db: state.db,
    dbName: "credit_card_accounts",
    constraints: {
      account_id: creditCardAccountId,
    },
  });
  if (ccAccounts.length === 0) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  const [success] = updateRecordInDb({
    db: state.db,
    dbName: "credit_card_accounts",
    recordId: creditCardAccountId,
    updates: {
      status: "CLOSED",
      closed_date: getTodayStr(),
      closed_by: userId,
    },
  });
  if (!success) {
    return `Error: Failed to close credit card account '${creditCardAccountId}'.`;
  }
  return (
    `Credit card account closed successfully.\n\n` +
    `Executed: close_credit_card_account_7834\n` +
    `Arguments: ${JSON.stringify({ credit_card_account_id: creditCardAccountId, user_id: userId }, null, 2)}\n` +
    `Account ${creditCardAccountId} has been closed.`
  );
}

const closeCreditCardAccountTool: DiscoverableTool = {
  name: "close_credit_card_account_7834",
  description: "Close a customer's credit card account permanently.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The credit card account ID to close",
    },
    {
      name: "user_id",
      type: "string",
      optional: false,
      description: "The user's unique identifier in the system",
    },
  ] satisfies readonly ToolParameter[],
  handler: closeCreditCardAccount,
};

export function registerCreditTools(): void {
  registerDiscoverableAgentTool(orderReplacementCreditCardTool);
  registerDiscoverableAgentTool(getPendingReplacementOrdersTool);
  registerDiscoverableAgentTool(logCreditCardClosureReasonTool);
  registerDiscoverableAgentTool(closeCreditCardAccountTool);
  registerDiscoverableAgentTool(getUserDisputeHistoryTool);
  registerDiscoverableAgentTool(getClosureReasonHistoryTool);
  registerDiscoverableAgentTool(applyStatementCreditTool);
  registerDiscoverableAgentTool(applyCreditCardAccountFlagTool);
  registerCreditLimitAndPaymentTools();
}
