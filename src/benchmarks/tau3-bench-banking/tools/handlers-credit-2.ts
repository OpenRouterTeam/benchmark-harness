import { queryDb, addToDb } from "./db-query";
import { getAccountBalance, getTodayStr, parseBalance } from "./helpers";
import { generateCreditLimitIncreaseRequestId } from "./ids";
import type {
  BankingEnvState,
  DiscoverableTool,
  ToolParameter,
} from "./registry";
import { registerDiscoverableAgentTool } from "./registry";

function formatUsd(amount: number): string {
  const formatted = amount.toFixed(2);
  const parts = formatted.split(".");
  const intPart = parts[0]!;
  const decPart = parts[1]!;
  const withCommas = intPart.replaceAll(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${withCommas}.${decPart}`;
}

function payCreditCardFromChecking(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = String(kwargs.user_id ?? "");
  const checkingAccountId = String(kwargs.checking_account_id ?? "");
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  const amountRaw = kwargs.amount;
  if (
    !userId ||
    !checkingAccountId ||
    !creditCardAccountId ||
    amountRaw === null ||
    amountRaw === undefined
  ) {
    return "Error: Missing required parameters (user_id, checking_account_id, credit_card_account_id, amount).";
  }
  const amount =
    typeof amountRaw === "number"
      ? amountRaw
      : Number.parseFloat(String(amountRaw));
  if (Number.isNaN(amount)) {
    return "Error: Invalid payment amount. Must be a positive number.";
  }
  if (amount <= 0) {
    return "Error: Payment amount must be a positive number.";
  }
  const accountsTable = state.db.accounts;
  if (!(checkingAccountId in accountsTable.data)) {
    return `Error: Checking account '${checkingAccountId}' not found.`;
  }
  const checkingAccount = accountsTable.data[checkingAccountId];
  if (!checkingAccount) {
    return `Error: Checking account '${checkingAccountId}' not found.`;
  }
  if (checkingAccount.user_id !== userId) {
    return `Error: Checking account '${checkingAccountId}' does not belong to user '${userId}'.`;
  }
  if (checkingAccount.class !== "checking") {
    return `Error: Account '${checkingAccountId}' is not a checking account.`;
  }
  const currentBalance = getAccountBalance(checkingAccount);
  if (amount > currentBalance) {
    return `Error: Insufficient funds in checking account. Available balance: $${currentBalance.toFixed(2)}, requested payment: $${amount.toFixed(2)}.`;
  }
  const ccAccountsTable = state.db.credit_card_accounts;
  if (!(creditCardAccountId in ccAccountsTable.data)) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  const ccAccount = ccAccountsTable.data[creditCardAccountId];
  if (!ccAccount) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  if (ccAccount.user_id !== userId) {
    return `Error: Credit card account '${creditCardAccountId}' does not belong to user '${userId}'.`;
  }
  const ccBalanceStr = String(ccAccount.current_balance ?? "$0.00");
  const ccBalance = parseBalance(ccBalanceStr);
  if (amount > ccBalance) {
    return `Error: Payment amount ($${amount.toFixed(2)}) exceeds credit card balance ($${ccBalance.toFixed(2)}). Please specify an amount up to the outstanding balance.`;
  }
  const newCheckingBalance = currentBalance - amount;
  const newCcBalance = ccBalance - amount;
  checkingAccount.current_holdings = newCheckingBalance.toFixed(2);
  ccAccount.current_balance = `$${newCcBalance.toFixed(2)}`;
  return (
    `Payment processed successfully!\n` +
    `  - Payment Amount: $${amount.toFixed(2)}\n` +
    `  - From Checking Account: ${checkingAccountId}\n` +
    `  - To Credit Card Account: ${creditCardAccountId}\n` +
    `  - New Checking Balance: $${newCheckingBalance.toFixed(2)}\n` +
    `  - New Credit Card Balance: $${newCcBalance.toFixed(2)}\n` +
    `The payment has been applied immediately.`
  );
}

const payCreditCardFromCheckingTool: DiscoverableTool = {
  name: "pay_credit_card_from_checking_9182",
  description:
    "Pay off a credit card balance using funds from the customer's Rho-Bank checking account. This deducts the specified amount from the checking account and reduces the credit card balance by the same amount.",
  params: [
    {
      name: "user_id",
      type: "string",
      optional: false,
      description: "The customer's unique identifier in the system",
    },
    {
      name: "checking_account_id",
      type: "string",
      optional: false,
      description:
        "The ID of the Rho-Bank checking account to debit funds from",
    },
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The ID of the credit card account to apply the payment to",
    },
    {
      name: "amount",
      type: "number",
      optional: false,
      description: "The payment amount in dollars. Must be a positive number.",
    },
  ] satisfies readonly ToolParameter[],
  handler: payCreditCardFromChecking,
};

function submitCreditLimitIncreaseRequest(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  const userId = String(kwargs.user_id ?? "");
  const requestedIncreaseAmountRaw = kwargs.requested_increase_amount;
  if (
    !creditCardAccountId ||
    !userId ||
    requestedIncreaseAmountRaw === null ||
    requestedIncreaseAmountRaw === undefined
  ) {
    return "Error: Missing required parameters.";
  }
  const requestedIncreaseAmountFloat =
    typeof requestedIncreaseAmountRaw === "number"
      ? requestedIncreaseAmountRaw
      : Number(String(requestedIncreaseAmountRaw));
  if (!Number.isFinite(requestedIncreaseAmountFloat)) {
    return "Error: Invalid requested_increase_amount. Must be a whole number.";
  }
  if (!Number.isInteger(requestedIncreaseAmountFloat)) {
    return "Error: Invalid requested_increase_amount. Must be a whole number of dollars.";
  }
  const requestedIncreaseAmount = requestedIncreaseAmountFloat;
  if (requestedIncreaseAmount <= 0) {
    return "Error: Requested increase amount must be positive.";
  }
  const ccAccountsTable = state.db.credit_card_accounts;
  if (!(creditCardAccountId in ccAccountsTable.data)) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  const ccAccount = ccAccountsTable.data[creditCardAccountId];
  if (!ccAccount) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  if (ccAccount.user_id !== userId) {
    return `Error: Credit card account '${creditCardAccountId}' does not belong to user '${userId}'.`;
  }
  const requestId = generateCreditLimitIncreaseRequestId(
    creditCardAccountId,
    userId,
    requestedIncreaseAmount
  );
  const today = getTodayStr();
  const requestRecord = {
    request_id: requestId,
    credit_card_account_id: creditCardAccountId,
    user_id: userId,
    requested_increase_amount: requestedIncreaseAmount,
    submitted_at: today,
    status: "PENDING",
  };
  const success = addToDb({
    db: state.db,
    dbName: "credit_limit_increase_requests",
    recordId: requestId,
    record: requestRecord,
  });
  if (!success) {
    return "Error: A similar request may already exist.";
  }
  return (
    `Credit limit increase request submitted successfully.\n\n` +
    `Executed: submit_credit_limit_increase_request_7392\n` +
    `  - Request ID: ${requestId}\n` +
    `  - Account: ${creditCardAccountId}\n` +
    `  - Requested Increase: $${formatUsd(requestedIncreaseAmount)}\n` +
    `  - Status: PENDING`
  );
}

const submitCreditLimitIncreaseRequestTool: DiscoverableTool = {
  name: "submit_credit_limit_increase_request_7392",
  description:
    "Submit a credit limit increase request for a customer's credit card.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The credit card account ID to request increase for",
    },
    {
      name: "user_id",
      type: "string",
      optional: false,
      description: "The customer's unique identifier in the system",
    },
    {
      name: "requested_increase_amount",
      type: "integer",
      optional: false,
      description:
        "The dollar amount by which to increase the credit limit (e.g., 2500 for $2,500)",
    },
  ] satisfies readonly ToolParameter[],
  handler: submitCreditLimitIncreaseRequest,
};

function getCreditLimitIncreaseHistory(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  if (!creditCardAccountId) {
    return "Error: Missing required parameter: credit_card_account_id";
  }
  const cliRequests = queryDb({
    db: state.db,
    dbName: "credit_limit_increase_requests",
    constraints: {
      credit_card_account_id: creditCardAccountId,
    },
  });
  const resultParts = [
    "Credit limit increase history retrieved.",
    "",
    "Executed: get_credit_limit_increase_history_4829",
    `Credit limit increase history for account ${creditCardAccountId}:`,
  ];
  if (cliRequests.length === 0) {
    resultParts.push(
      "\nNo credit limit increase requests found for this account."
    );
  } else {
    resultParts.push("");
    for (const [index, record] of cliRequests.entries()) {
      resultParts.push(`${index + 1}. Request ID: ${record.request_id}`);
      for (const [field, value] of Object.entries(record)) {
        resultParts.push(`   ${field}: ${value}`);
      }
      resultParts.push("");
    }
  }
  return resultParts.join("\n");
}

const getCreditLimitIncreaseHistoryTool: DiscoverableTool = {
  name: "get_credit_limit_increase_history_4829",
  mutatesState: false,
  description:
    "Retrieve the credit limit increase request history for a specific credit card account. Returns all previous CLI requests including dates, amounts, and statuses.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The credit card account ID to check for CLI history",
    },
  ] satisfies readonly ToolParameter[],
  handler: getCreditLimitIncreaseHistory,
};

function getPaymentHistory(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  const monthsRaw = kwargs.months;
  if (!creditCardAccountId || monthsRaw === null || monthsRaw === undefined) {
    return "Error: Missing required parameters (credit_card_account_id, months).";
  }
  const months = parsePythonInt(monthsRaw);
  if (months === undefined) {
    return "Error: Invalid months value. Must be a positive integer.";
  }
  if (months <= 0) {
    return "Error: months must be a positive integer.";
  }
  const paymentTable = state.db.payment_history;
  const payments: Record<string, unknown>[] = [];
  for (const paymentData of Object.values(paymentTable.data)) {
    if (paymentData.credit_card_account_id === creditCardAccountId) {
      payments.push(paymentData);
    }
  }
  if (payments.length === 0) {
    return `No payment history found for account '${creditCardAccountId}'.`;
  }
  payments.sort((a, b) => {
    const dateA = String(a.payment_date ?? "");
    const dateB = String(b.payment_date ?? "");
    return dateB.localeCompare(dateA);
  });
  const limitedPayments = payments.slice(0, months);
  let consecutiveOnTime = 0;
  for (const payment of limitedPayments) {
    if (payment.status === "ON_TIME") {
      consecutiveOnTime += 1;
    } else {
      break;
    }
  }
  const resultParts = [
    `Payment history for account '${creditCardAccountId}' (last ${months} months):`,
    `Consecutive on-time payments: ${consecutiveOnTime}`,
  ];
  for (const payment of limitedPayments) {
    resultParts.push(
      `\n  - Payment Date: ${payment.payment_date}\n` +
        `    Amount: ${payment.amount}\n` +
        `    Status: ${payment.status}`
    );
  }
  return resultParts.join("\n");
}

function parsePythonInt(value: unknown): number | undefined {
  if (typeof value === "number") {
    return Number.isFinite(value) ? Math.trunc(value) : undefined;
  }
  if (typeof value === "boolean") {
    return value ? 1 : 0;
  }
  if (typeof value !== "string" || !/^[+-]?\d+$/u.test(value.trim())) {
    return undefined;
  }
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? parsed : undefined;
}

const getPaymentHistoryTool: DiscoverableTool = {
  name: "get_payment_history_6183",
  mutatesState: false,
  description: "Retrieve payment history for a credit card account.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The credit card account ID to check payment history for",
    },
    {
      name: "months",
      type: "integer",
      optional: false,
      description: "Number of months of payment history to retrieve",
    },
  ] satisfies readonly ToolParameter[],
  handler: getPaymentHistory,
};

function approveCreditLimitIncrease(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  const userId = String(kwargs.user_id ?? "");
  const newCreditLimitRaw = kwargs.new_credit_limit;
  if (
    !creditCardAccountId ||
    !userId ||
    newCreditLimitRaw === null ||
    newCreditLimitRaw === undefined
  ) {
    return "Error: Missing required parameters.";
  }
  let newCreditLimit: number;
  try {
    newCreditLimit =
      typeof newCreditLimitRaw === "number"
        ? newCreditLimitRaw
        : Number.parseInt(String(newCreditLimitRaw), 10);
  } catch {
    return "Error: Invalid new credit limit.";
  }
  const ccAccountsTable = state.db.credit_card_accounts;
  if (!(creditCardAccountId in ccAccountsTable.data)) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  const ccAccount = ccAccountsTable.data[creditCardAccountId];
  if (!ccAccount) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  if (ccAccount.user_id !== userId) {
    return `Error: Credit card account '${creditCardAccountId}' does not belong to user '${userId}'.`;
  }
  let ineligible = false;
  if (state.db.transaction_disputes && state.db.transaction_disputes.data) {
    for (const dispute of Object.values(state.db.transaction_disputes.data)) {
      if (
        dispute.user_id === userId &&
        ["SUBMITTED", "UNDER_REVIEW", "PENDING"].includes(
          String(dispute.status)
        )
      ) {
        ineligible = true;
        break;
      }
    }
  }
  if (
    !ineligible &&
    state.db.credit_card_orders &&
    state.db.credit_card_orders.data
  ) {
    for (const order of Object.values(state.db.credit_card_orders.data)) {
      if (
        order.credit_card_account_id === creditCardAccountId &&
        ["PENDING", "PROCESSING", "SHIPPED"].includes(String(order.status))
      ) {
        ineligible = true;
        break;
      }
    }
  }
  if (!ineligible) {
    const accountStatus = String(ccAccount.account_status ?? "").toUpperCase();
    if (
      ["PAST_DUE", "DELINQUENT", "COLLECTIONS", "CLOSED"].includes(
        accountStatus
      )
    ) {
      ineligible = true;
    }
  }
  if (ineligible) {
    return "Error: Credit limit increase request cannot be approved at this time.";
  }
  const currentLimitStr = String(ccAccount.credit_limit ?? "$0.00");
  const currentLimit = parseBalance(currentLimitStr);
  const newLimit = Number.parseFloat(String(newCreditLimit));
  ccAccount.credit_limit = `$${newLimit.toFixed(2)}`;
  const today = getTodayStr();
  const requestId = generateCreditLimitIncreaseRequestId(
    creditCardAccountId,
    userId,
    newLimit - currentLimit
  );
  const approvalRecord = {
    request_id: requestId,
    credit_card_account_id: creditCardAccountId,
    user_id: userId,
    previous_limit: `$${currentLimit.toFixed(2)}`,
    new_limit: `$${newLimit.toFixed(2)}`,
    increase_amount: `$${(newLimit - currentLimit).toFixed(2)}`,
    decision_date: today,
    status: "APPROVED",
  };
  addToDb({
    db: state.db,
    dbName: "credit_limit_increase_requests",
    recordId: requestId,
    record: approvalRecord,
  });
  return (
    `Credit limit increase approved!\n` +
    `  - Account: ${creditCardAccountId}\n` +
    `  - Previous Limit: $${currentLimit.toFixed(2)}\n` +
    `  - New Limit: $${newLimit.toFixed(2)}\n` +
    `  - Increase: $${(newLimit - currentLimit).toFixed(2)}\n` +
    `  - Effective Date: ${today}\n` +
    `The customer will receive a confirmation email.`
  );
}

const approveCreditLimitIncreaseTool: DiscoverableTool = {
  name: "approve_credit_limit_increase_5847",
  description:
    "Approve and apply a credit limit increase for a customer's credit card.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The credit card account ID",
    },
    {
      name: "user_id",
      type: "string",
      optional: false,
      description: "The customer's unique identifier in the system",
    },
    {
      name: "new_credit_limit",
      type: "integer",
      optional: false,
      description:
        "The new total credit limit in dollars (e.g., 7500 for $7,500)",
    },
  ] satisfies readonly ToolParameter[],
  handler: approveCreditLimitIncrease,
};

function denyCreditLimitIncrease(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = String(kwargs.credit_card_account_id ?? "");
  const userId = String(kwargs.user_id ?? "");
  const denialReason = String(kwargs.denial_reason ?? "");
  if (!creditCardAccountId || !userId || !denialReason) {
    return "Error: Missing required parameters.";
  }
  const validReasons = [
    "insufficient_account_age",
    "cooldown_period_active",
    "pending_disputes",
    "pending_replacement_card",
    "past_due_balance",
    "high_utilization",
    "insufficient_payment_history",
    "requested_amount_exceeds_limit",
    "other",
  ];
  if (!validReasons.includes(denialReason)) {
    return `Error: Invalid denial_reason. Must be one of: ${validReasons.join(", ")}`;
  }
  const ccAccountsTable = state.db.credit_card_accounts;
  if (!(creditCardAccountId in ccAccountsTable.data)) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  const today = getTodayStr();
  const requestId = generateCreditLimitIncreaseRequestId(
    creditCardAccountId,
    userId,
    0
  );
  const denialRecord = {
    request_id: requestId,
    credit_card_account_id: creditCardAccountId,
    user_id: userId,
    denial_reason: denialReason,
    decision_date: today,
    status: "DENIED",
  };
  addToDb({
    db: state.db,
    dbName: "credit_limit_increase_requests",
    recordId: requestId,
    record: denialRecord,
  });
  return (
    `Credit limit increase request denied.\n` +
    `  - Account: ${creditCardAccountId}\n` +
    `  - Denial Reason: ${denialReason}\n` +
    `  - Date: ${today}\n` +
    `The customer will receive a notification explaining the denial.`
  );
}

const denyCreditLimitIncreaseTool: DiscoverableTool = {
  name: "deny_credit_limit_increase_5848",
  description:
    "Deny a credit limit increase request for a customer's credit card.",
  params: [
    {
      name: "credit_card_account_id",
      type: "string",
      optional: false,
      description: "The credit card account ID",
    },
    {
      name: "user_id",
      type: "string",
      optional: false,
      description: "The customer's unique identifier in the system",
    },
    {
      name: "denial_reason",
      type: "string",
      optional: false,
      description:
        "The reason for denying the request. Must be one of: 'insufficient_account_age', 'cooldown_period_active', 'pending_disputes', 'pending_replacement_card', 'past_due_balance', 'high_utilization', 'insufficient_payment_history', 'requested_amount_exceeds_limit', 'other'",
    },
  ] satisfies readonly ToolParameter[],
  handler: denyCreditLimitIncrease,
};

export function registerCreditLimitAndPaymentTools(): void {
  registerDiscoverableAgentTool(payCreditCardFromCheckingTool);
  registerDiscoverableAgentTool(submitCreditLimitIncreaseRequestTool);
  registerDiscoverableAgentTool(getCreditLimitIncreaseHistoryTool);
  registerDiscoverableAgentTool(getPaymentHistoryTool);
  registerDiscoverableAgentTool(approveCreditLimitIncreaseTool);
  registerDiscoverableAgentTool(denyCreditLimitIncreaseTool);
}
