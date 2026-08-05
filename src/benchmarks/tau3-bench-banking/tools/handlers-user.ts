import { createHash } from "node:crypto";

import type { ToolDefinition } from "../../../harness/core";
import { unknownErrorToString } from "../../../internal/errors";
import { isRecord } from "../../../internal/guards";
import { addToDb, queryDatabaseTool } from "./db-query";
import {
  getFieldAsString,
  getFieldAsBoolean,
  getFieldAsRecord,
} from "./field-access";
import { FROZEN_TODAY_STR as FROZEN_DATE, parseBalance } from "./helpers";
import {
  generateApplicationId,
  generateDisputeId,
  generateReferralId,
  generateReferralLinkId,
  generateTransactionId,
  generateUserDiscoverableToolCallId,
} from "./ids";
import type { BankingEnvState, DiscoverableTool } from "./registry";
import {
  registerDiscoverableUserTool,
  DISCOVERABLE_USER_TOOLS,
} from "./registry";

const VALID_CREDIT_CARD_TYPES = [
  "Bronze Rewards Card",
  "Business Bronze Rewards Card",
  "Business Gold Rewards Card",
  "Business Platinum Rewards Card",
  "Business Silver Rewards Card",
  "Crypto-Cash Back",
  "Diamond Elite Card",
  "EcoCard",
  "Gold Rewards Card",
  "Green Rewards Card",
  "Platinum Rewards Card",
  "Silver Rewards Card",
  "Silver Zoom Card",
] as const;

const VALID_CREDIT_CARD_TYPE_SET: ReadonlySet<string> = new Set(
  VALID_CREDIT_CARD_TYPES
);

const CREDIT_CARD_REWARDS: Record<string, Record<string, number>> = {
  "Bronze Rewards Card": { default: 1 },
  "Silver Rewards Card": { Travel: 4, Software: 4, default: 1 },
  "Gold Rewards Card": { default: 2.5 },
  "Platinum Rewards Card": { default: 10 },
  "Business Bronze Rewards Card": { default: 1 },
  "Business Silver Rewards Card": { Travel: 10, Software: 10, default: 1 },
  "Green Rewards Card": { Sustainable: 3, default: 1 },
  "Business Gold Rewards Card": { Operations: 2.5, default: 1 },
  "Business Platinum Rewards Card": {
    Travel: 4,
    Software: 4,
    Media: 4,
    default: 1.5,
  },
  "Silver Zoom Card": { Transportation: 3, default: 1 },
  "Diamond Elite Card": { default: 5 },
  EcoCard: { Green: 5, default: 1 },
  "Crypto-Cash Back": { default: 2 },
};

function checkToolGiven(
  state: BankingEnvState,
  toolName: string
): string | null {
  const result = queryDatabaseTool(
    state.db,
    "user_discoverable_tools",
    JSON.stringify({ tool_name: toolName })
  );
  if (result.includes("No records found")) {
    return `Error: Tool '${toolName}' has not been given to you by the agent. The agent must first use \`give_discoverable_user_tool\` to give this tool to you.`;
  }
  return null;
}

function logUserToolCall(
  state: BankingEnvState,
  toolName: string,
  args: Record<string, unknown>
): void {
  const callRecord = {
    tool_name: toolName,
    arguments: args,
    called_at: FROZEN_DATE,
    status: "CALLED",
  };
  const callRecordId = generateUserDiscoverableToolCallId(toolName, args);
  addToDb({
    db: state.db,
    dbName: "user_discoverable_tool_calls",
    recordId: callRecordId,
    record: callRecord,
  });
}

function submitCashBackDispute(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const error = checkToolGiven(state, "submit_cash_back_dispute_0589");
  if (error) {
    return error;
  }
  const userId = getFieldAsString(kwargs, "user_id");
  const transactionId = getFieldAsString(kwargs, "transaction_id");
  if (!userId || !transactionId) {
    return "Error: Missing required parameters: user_id, transaction_id";
  }
  const args = { user_id: userId, transaction_id: transactionId };
  logUserToolCall(state, "submit_cash_back_dispute_0589", args);
  const disputeId = generateDisputeId(userId, transactionId);
  let autoResolve = false;
  if (state.db.task_config?.data) {
    const config =
      getFieldAsRecord(state.db.task_config.data, "dispute_settings") ?? {};
    autoResolve = getFieldAsBoolean(config, "auto_resolve_disputes");
  }
  let statusMsg: string;
  if (autoResolve) {
    const disputeRecord = {
      dispute_id: disputeId,
      user_id: userId,
      transaction_id: transactionId,
      submitted_at: FROZEN_DATE,
      status: "RESOLVED",
      resolution: "APPROVED",
    };
    addToDb({
      db: state.db,
      dbName: "cash_back_disputes",
      recordId: disputeId,
      record: disputeRecord,
    });
    statusMsg =
      "Status: RESOLVED - The dispute has been reviewed and approved. The transaction rewards need to be updated.";
  } else {
    const disputeRecord = {
      dispute_id: disputeId,
      user_id: userId,
      transaction_id: transactionId,
      submitted_at: FROZEN_DATE,
      status: "SUBMITTED",
    };
    addToDb({
      db: state.db,
      dbName: "cash_back_disputes",
      recordId: disputeId,
      record: disputeRecord,
    });
    statusMsg = "Status: SUBMITTED - Your dispute has been queued for review.";
  }
  const result =
    `Cash back dispute submitted successfully. Your case has been queued for review.\n\n` +
    `Executed: submit_cash_back_dispute_0589\n` +
    `Arguments: ${JSON.stringify(args, null, 2)}\n` +
    `Dispute ID: ${disputeId}\n` +
    `${statusMsg}`;
  return result;
}

function depositCheck(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const error = checkToolGiven(state, "deposit_check_3847");
  if (error) {
    return error;
  }
  const accountId = getFieldAsString(kwargs, "account_id");
  const checkAmountRaw = kwargs.check_amount;
  if (!accountId || checkAmountRaw === undefined) {
    return "Error: Missing required parameters: account_id, check_amount";
  }
  let checkAmount: number;
  if (typeof checkAmountRaw === "string") {
    const parsed = Number.parseFloat(checkAmountRaw);
    checkAmount = Number.isFinite(parsed) ? parsed : 0;
  } else if (typeof checkAmountRaw === "number") {
    checkAmount = checkAmountRaw;
  } else {
    checkAmount = 0;
  }
  if (!Number.isFinite(checkAmount) || checkAmount <= 0) {
    return "Error: Check amount must be a positive number.";
  }
  const args = { account_id: accountId, check_amount: checkAmount };
  logUserToolCall(state, "deposit_check_3847", args);
  if (!(accountId in (state.db.accounts?.data ?? {}))) {
    return `Error: Account '${accountId}' not found.`;
  }
  const account = state.db.accounts.data[accountId];
  if (!account) {
    return `Error: Account '${accountId}' not found.`;
  }
  const status = typeof account.status === "string" ? account.status : "";
  if (!["ACTIVE", "OPEN"].includes(status)) {
    return `Error: Account '${accountId}' is not active.`;
  }
  const currentBalance = parseBalance(
    account.current_holdings ?? account.balance ?? 0
  );
  const newBalance = currentBalance + checkAmount;
  state.db.accounts!.data[accountId] = {
    ...account,
    current_holdings: `$${newBalance.toFixed(2)}`,
  };
  const result =
    `Check deposited successfully. Funds will be available according to your account's deposit policy.\n\n` +
    `Executed: deposit_check_3847\n` +
    `Arguments: ${JSON.stringify(args, null, 2)}\n` +
    `Check deposit processed!\n` +
    `  - Account: ${accountId}\n` +
    `  - Check Amount: $${checkAmount.toFixed(2)}\n` +
    `  - Previous Balance: $${currentBalance.toFixed(2)}\n` +
    `  - New Balance: $${newBalance.toFixed(2)}`;
  return result;
}

function getCardLast4Digits(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const creditCardAccountId = getFieldAsString(
    kwargs,
    "credit_card_account_id"
  );
  if (!creditCardAccountId) {
    return "Error: Missing required parameter: credit_card_account_id";
  }
  const givenError = checkToolGiven(state, "get_card_last_4_digits");
  if (givenError) {
    return givenError;
  }
  logUserToolCall(state, "get_card_last_4_digits", {
    credit_card_account_id: creditCardAccountId,
  });
  const result = queryDatabaseTool(
    state.db,
    "credit_card_accounts",
    JSON.stringify({ account_id: creditCardAccountId })
  );
  if (result.includes("No records found") || result.includes("not found")) {
    return `Error: Credit card account '${creditCardAccountId}' not found.`;
  }
  const hashInput = `card_last4:${creditCardAccountId}`;
  const hashDigest = createHash("sha256")
    .update(hashInput, "utf8")
    .digest("hex");
  let last4 = "";
  for (const char of hashDigest) {
    if (/\d/.test(char)) {
      last4 += char;
      if (last4.length === 4) {
        break;
      }
    }
  }
  last4 = last4.padEnd(4, "0");
  const args = { credit_card_account_id: creditCardAccountId };
  const result2 =
    `Card information retrieved successfully.\n\n` +
    `Executed: get_card_last_4_digits\n` +
    `Arguments: ${JSON.stringify(args, null, 2)}\n` +
    `Last 4 digits of card: ${last4}`;
  return result2;
}

function getReferralLink(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = getFieldAsString(kwargs, "user_id");
  const cardName = getFieldAsString(kwargs, "card_name");
  if (!userId || !cardName) {
    return "Error: Missing required parameters: user_id, card_name";
  }
  const givenError = checkToolGiven(state, "get_referral_link");
  if (givenError) {
    return givenError;
  }
  const args = { user_id: userId, card_name: cardName };
  logUserToolCall(state, "get_referral_link", args);
  const referralId = generateReferralLinkId(userId, cardName);
  const referralRecord = {
    referral_id: referralId,
    referrer_id: userId,
    referred_account_type: cardName,
    referral_status: "NO_PROGRESS",
    date: FROZEN_DATE,
  };
  const success = addToDb({
    db: state.db,
    dbName: "referrals",
    recordId: referralId,
    record: referralRecord,
  });
  const result =
    `Referral link generated successfully. Share this link with the person you want to refer.\n\n` +
    `Executed: get_referral_link\n` +
    `Arguments: ${JSON.stringify(args, null, 2)}\n${
      success
        ? `Referral ID: ${referralId}\nReferral link: https://rhobank.com/refer/${referralId}`
        : "Note: A referral link for this card may have already been generated."
    }`;
  return result;
}

function applyForCreditCard(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const cardType = getFieldAsString(kwargs, "card_type");
  const customerName = getFieldAsString(kwargs, "customer_name");
  const annualIncomeRaw = kwargs.annual_income;
  const rhoBankSubscription = getFieldAsBoolean(
    kwargs,
    "rho_bank_subscription"
  );
  if (!cardType || !customerName || annualIncomeRaw === undefined) {
    return "Error: Missing required parameters: card_type, customer_name, annual_income";
  }
  if (!VALID_CREDIT_CARD_TYPE_SET.has(cardType)) {
    return `Error: Invalid card_type '${cardType}'. Must be one of: ${VALID_CREDIT_CARD_TYPES.join(", ")}`;
  }
  let annualIncome = Number.NaN;
  if (typeof annualIncomeRaw === "number") {
    annualIncome = annualIncomeRaw;
  } else if (typeof annualIncomeRaw === "string") {
    annualIncome = Number(annualIncomeRaw);
  }
  if (!Number.isFinite(annualIncome)) {
    return "Error: Invalid annual_income. Must be a number.";
  }
  const applicationId = generateApplicationId({
    cardType,
    customerName,
    annualIncome,
    rhoaBankSubscription: rhoBankSubscription,
  });
  const record = {
    application_id: applicationId,
    card_type: cardType,
    customer_name: customerName,
    annual_income: annualIncome,
    rho_bank_subscription: rhoBankSubscription,
    status: "PENDING",
    date: FROZEN_DATE,
  };
  const success = addToDb({
    db: state.db,
    dbName: "credit_card_applications",
    recordId: applicationId,
    record,
  });
  if (!success) {
    return `Failed to submit application: Record ID '${applicationId}' may already exist.`;
  }
  return "Credit card application submitted:\nYour application has been successfully submitted. You will receive a decision within 5-7 business days via email.";
}

function submitReferral(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = getFieldAsString(kwargs, "user_id");
  const accountType = getFieldAsString(kwargs, "account_type");
  if (!userId || !accountType) {
    return "Error: Missing required parameters: user_id, account_type";
  }
  const referralId = generateReferralId(userId, accountType);
  const record = {
    referral_id: referralId,
    referrer_id: userId,
    referred_account_type: accountType,
    referral_status: "NO_PROGRESS",
    date: FROZEN_DATE,
  };
  const success = addToDb({
    db: state.db,
    dbName: "referrals",
    recordId: referralId,
    record,
  });
  if (!success) {
    return `Failed to submit referral: Record ID '${referralId}' may already exist.`;
  }
  return (
    `Referral request submitted successfully!\n` +
    `  - Referral ID: ${referralId}\n` +
    `  - Referrer ID: ${userId}\n` +
    `  - Account Type: ${accountType}\n` +
    `  - Status: NO_PROGRESS\n` +
    `  - Date: ${FROZEN_DATE}\n\n` +
    `Share your referral ID with the person you're referring. They will need to use this when applying for their account.`
  );
}

function queryDatabase(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const databaseName = getFieldAsString(kwargs, "database_name");
  const constraints = getFieldAsString(kwargs, "constraints") ?? "{}";
  if (!databaseName) {
    return "Error: Missing required parameter: database_name";
  }
  return queryDatabaseTool(state.db, databaseName, constraints);
}

function callDiscoverableUserTool(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const toolName = getFieldAsString(kwargs, "discoverable_tool_name");
  const argumentsStr = getFieldAsString(kwargs, "arguments") ?? "{}";
  if (!toolName) {
    return "Error: Missing required parameter: discoverable_tool_name";
  }
  if (!DISCOVERABLE_USER_TOOLS.has(toolName)) {
    return `Error: Unknown discoverable tool '${toolName}'.`;
  }
  let argsDict: Record<string, unknown> = {};
  try {
    if (argumentsStr) {
      const parsed: unknown = JSON.parse(argumentsStr);
      if (!isRecord(parsed)) {
        return "Error: Arguments must be a JSON object";
      }
      argsDict = parsed;
    }
  } catch (error) {
    return `Error: Invalid JSON in arguments: ${unknownErrorToString(error)}`;
  }
  const tool = DISCOVERABLE_USER_TOOLS.get(toolName)!;
  try {
    return tool.handler(state, argsDict);
  } catch (error) {
    return `Error: Failed to call tool '${toolName}': ${unknownErrorToString(error)}`;
  }
}

function listDiscoverableUserTools(
  state: BankingEnvState,
  _kwargs: Record<string, unknown>
): string {
  const result = queryDatabaseTool(state.db, "user_discoverable_tools", "{}");
  if (result.includes("No records found")) {
    return "No tools have been given to you yet by the agent.";
  }
  return `Tools given to you by the agent:\n${result}`;
}

function requestHumanAgentTransfer(
  state: BankingEnvState,
  _kwargs: Record<string, unknown>
): string {
  const today = FROZEN_DATE;
  const existingRequests = queryDatabaseTool(
    state.db,
    "human_transfer_requests",
    "{}"
  );
  let requestCount = 1;
  if (!existingRequests.includes("No records found")) {
    const matchCount = (existingRequests.match(/request_id/g) ?? []).length;
    requestCount = matchCount + 1;
  }
  const requestId = `transfer_request_${requestCount}`;
  const record = {
    request_id: requestId,
    request_number: requestCount,
    requested_at: today,
    status: "PENDING",
  };
  addToDb({
    db: state.db,
    dbName: "human_transfer_requests",
    recordId: requestId,
    record,
  });
  return `Transfer request #${requestCount} submitted.\nThe agent will process your request.`;
}

function submitTransaction(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = getFieldAsString(kwargs, "user_id");
  const creditCardType = getFieldAsString(kwargs, "credit_card_type");
  const merchantName = getFieldAsString(kwargs, "merchant_name");
  const amountRaw = kwargs.amount;
  const category = getFieldAsString(kwargs, "category");
  if (
    !userId ||
    !creditCardType ||
    !merchantName ||
    amountRaw === undefined ||
    !category
  ) {
    return "Error: Missing required parameters: user_id, credit_card_type, merchant_name, amount, category";
  }
  if (!VALID_CREDIT_CARD_TYPE_SET.has(creditCardType)) {
    const available = Object.keys(CREDIT_CARD_REWARDS);
    return `Error: Unknown credit card type '${creditCardType}'. Available types: ${available}`;
  }
  let amount: number;
  if (typeof amountRaw === "string") {
    const parsed = Number.parseFloat(amountRaw);
    amount = Number.isFinite(parsed) ? parsed : 0;
  } else if (typeof amountRaw === "number") {
    amount = amountRaw;
  } else {
    amount = 0;
  }
  if (!Number.isFinite(amount) || amount <= 0) {
    return "Error: amount must be a positive number.";
  }
  const transactionId = generateTransactionId({
    userId,
    creditCardType,
    merchantName,
    amount,
    category,
  });
  const cardRewards = CREDIT_CARD_REWARDS[creditCardType] ?? { default: 1 };
  const rawRate = cardRewards[category];
  const rewardRate =
    typeof rawRate === "number" ? rawRate : (cardRewards.default ?? 1);
  const pointsEarned = Math.floor(amount * rewardRate);
  const record = {
    transaction_id: transactionId,
    user_id: userId,
    credit_card_type: creditCardType,
    merchant_name: merchantName,
    transaction_amount: `$${amount.toFixed(2)}`,
    transaction_date: FROZEN_DATE,
    category,
    status: "COMPLETED",
    rewards_earned: `${pointsEarned} points`,
  };
  const success = addToDb({
    db: state.db,
    dbName: "credit_card_transaction_history",
    recordId: transactionId,
    record,
  });
  if (!success) {
    return `Failed to submit transaction: Record ID '${transactionId}' may already exist.`;
  }
  return (
    `Transaction submitted successfully!\n` +
    `  - Transaction ID: ${transactionId}\n` +
    `  - User ID: ${userId}\n` +
    `  - Card Type: ${creditCardType}\n` +
    `  - Merchant: ${merchantName}\n` +
    `  - Amount: $${amount.toFixed(2)}\n` +
    `  - Category: ${category}\n` +
    `  - Date: ${FROZEN_DATE}\n` +
    `  - Rewards Earned: ${pointsEarned} points (${rewardRate}% cashback rate)\n`
  );
}

export function registerUserTools(): void {
  const tool1: DiscoverableTool = {
    name: "submit_cash_back_dispute_0589",
    description:
      "Submit a cash back dispute for a specific transaction. Dispute ID and status will be returned.",
    params: [
      {
        name: "user_id",
        type: "string",
        optional: false,
        description: "The user ID",
      },
      {
        name: "transaction_id",
        type: "string",
        optional: false,
        description: "The transaction ID",
      },
    ],
    handler: submitCashBackDispute,
  };
  registerDiscoverableUserTool(tool1);
  const tool2: DiscoverableTool = {
    name: "deposit_check_3847",
    description:
      "Deposit a check into a checking or savings account via mobile banking.",
    params: [
      {
        name: "account_id",
        type: "string",
        optional: false,
        description: "The account ID",
      },
      {
        name: "check_amount",
        type: "number",
        optional: false,
        description: "The check amount in USD",
      },
    ],
    handler: depositCheck,
  };
  registerDiscoverableUserTool(tool2);
  const tool3: DiscoverableTool = {
    name: "get_referral_link",
    description:
      "Generate a referral link for a specific credit card to share with friends or family.",
    params: [
      {
        name: "user_id",
        type: "string",
        optional: false,
        description: "The referrer user ID",
      },
      {
        name: "card_name",
        type: "string",
        optional: false,
        description: "The credit card name",
      },
    ],
    handler: getReferralLink,
  };
  registerDiscoverableUserTool(tool3);
  const tool4: DiscoverableTool = {
    name: "get_card_last_4_digits",
    description: "Look up the last 4 digits of a credit card number.",
    params: [
      {
        name: "credit_card_account_id",
        type: "string",
        optional: false,
        description: "The credit card account ID",
      },
    ],
    handler: getCardLast4Digits,
  };
  registerDiscoverableUserTool(tool4);
}

export const USER_PERMANENT_TOOL_DEFINITIONS: readonly ToolDefinition[] = [
  {
    type: "function",
    function: {
      name: "apply_for_credit_card",
      description: "Apply for a credit card.",
      parameters: {
        type: "object",
        properties: {
          card_type: { type: "string", description: "Type of credit card" },
          customer_name: { type: "string", description: "Full legal name" },
          annual_income: {
            type: "number",
            description: "Annual income in USD",
          },
          rho_bank_subscription: {
            type: "boolean",
            description: "Whether user has Rho-Bank+ subscription",
          },
        },
        required: ["card_type", "customer_name", "annual_income"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "submit_referral",
      description:
        "Submit a referral request to refer someone to open an account.",
      parameters: {
        type: "object",
        properties: {
          user_id: {
            type: "string",
            description: "Your user ID (the referrer)",
          },
          account_type: {
            type: "string",
            description:
              "The type of account you are referring someone to open",
          },
        },
        required: ["user_id", "account_type"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "query_database",
      description: "Query a database with constraints.",
      parameters: {
        type: "object",
        properties: {
          database_name: {
            type: "string",
            description: "Name of the database to query",
          },
          constraints: {
            type: "string",
            description: "JSON string of field constraints",
          },
        },
        required: ["database_name"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "call_discoverable_user_tool",
      description: "Call a tool that was given to you by the agent.",
      parameters: {
        type: "object",
        properties: {
          discoverable_tool_name: {
            type: "string",
            description: "The name of the discoverable tool",
          },
          arguments: {
            type: "string",
            description: "JSON string of arguments for the tool",
          },
        },
        required: ["discoverable_tool_name"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "list_discoverable_user_tools",
      description: "List all tools that have been given to you by the agent.",
      parameters: {
        type: "object",
        properties: {},
      },
    },
  },
  {
    type: "function",
    function: {
      name: "request_human_agent_transfer",
      description: "Request to be transferred to a human agent for assistance.",
      parameters: {
        type: "object",
        properties: {},
      },
    },
  },
  {
    type: "function",
    function: {
      name: "submit_transaction",
      description: "Submit a credit card transaction.",
      parameters: {
        type: "object",
        properties: {
          user_id: { type: "string", description: "Your user ID" },
          credit_card_type: {
            type: "string",
            description: "Type of credit card used",
          },
          merchant_name: {
            type: "string",
            description: "Name of the merchant",
          },
          amount: { type: "number", description: "Transaction amount in USD" },
          category: { type: "string", description: "Transaction category" },
        },
        required: [
          "user_id",
          "credit_card_type",
          "merchant_name",
          "amount",
          "category",
        ],
      },
    },
  },
];

export function invokeBankingUserTool(
  state: BankingEnvState,
  toolName: string,
  kwargs: Record<string, unknown>
): string {
  switch (toolName) {
    case "apply_for_credit_card": {
      return applyForCreditCard(state, kwargs);
    }
    case "submit_referral": {
      return submitReferral(state, kwargs);
    }
    case "query_database": {
      return queryDatabase(state, kwargs);
    }
    case "call_discoverable_user_tool": {
      return callDiscoverableUserTool(state, kwargs);
    }
    case "list_discoverable_user_tools": {
      return listDiscoverableUserTools(state, kwargs);
    }
    case "request_human_agent_transfer": {
      return requestHumanAgentTransfer(state, kwargs);
    }
    case "submit_transaction": {
      return submitTransaction(state, kwargs);
    }
    default: {
      return `Error: Unknown user tool '${toolName}'.`;
    }
  }
}
