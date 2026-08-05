import { createHash } from "node:crypto";

function deterministicId(seedString: string, length = 16): string {
  const hashBytes = createHash("sha256").update(seedString, "utf8").digest();
  return hashBytes.slice(0, length / 2).toString("hex");
}

export function pythonSortedJsonDumps(value: unknown): string {
  if (value === null || value === undefined) {
    return "null";
  }
  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }
  if (typeof value === "number" || typeof value === "string") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(pythonSortedJsonDumps).join(", ")}]`;
  }
  if (typeof value === "object") {
    const record: Record<string, unknown> = Object.fromEntries(
      Object.entries(value)
    );
    const entries = Object.keys(record)
      .toSorted()
      .map(
        (key) => `${JSON.stringify(key)}: ${pythonSortedJsonDumps(record[key])}`
      );
    return `{${entries.join(", ")}}`;
  }
  return "null";
}

export function generateTransactionId(opts: {
  userId: string;
  creditCardType: string;
  merchantName: string;
  amount: number;
  category: string;
  date?: string;
}): string {
  const { userId, creditCardType, merchantName, amount, category, date } = opts;
  const seedParts = [
    "transaction",
    userId,
    creditCardType,
    merchantName,
    amount.toFixed(2),
    category,
  ];
  if (date) {
    seedParts.push(date);
  }
  const seed = seedParts.join(":");
  return `txn_${deterministicId(seed, 12)}`;
}

export function generateReferralId(
  referrerId: string,
  referredAccountType: string,
  date?: string
): string {
  const seedParts = ["referral", referrerId, referredAccountType];
  if (date) {
    seedParts.push(date);
  }
  const seed = seedParts.join(":");
  return deterministicId(seed, 16);
}

export function generateApplicationId(opts: {
  cardType: string;
  customerName: string;
  annualIncome: number;
  rhoaBankSubscription?: boolean;
}): string {
  const {
    cardType,
    customerName,
    annualIncome,
    rhoaBankSubscription = false,
  } = opts;
  const boolStr = rhoaBankSubscription ? "True" : "False";
  const seed = `credit_card:${cardType}:${customerName}:${String(annualIncome)}:${boolStr}`;
  return deterministicId(seed, 16);
}

export function generateVerificationId(
  userId: string,
  timeVerified: string
): string {
  const timeSuffix = timeVerified
    .replaceAll(" ", "_")
    .replaceAll(":", "")
    .replaceAll("-", "");
  return `${userId}_${timeSuffix}`;
}

export function generateUserDiscoverableToolId(toolName: string): string {
  const seed = `user_discoverable_tool:${toolName}`;
  return deterministicId(seed, 16);
}

export function generateUserDiscoverableToolCallId(
  toolName: string,
  args: Record<string, unknown>
): string {
  const argsJson = pythonSortedJsonDumps(args);
  const seed = `user_discoverable_tool_call:${toolName}:${argsJson}`;
  return deterministicId(seed, 16);
}

export function generateDisputeId(
  userId: string,
  transactionId: string
): string {
  const seed = `dispute:${userId}:${transactionId}`;
  return `dsp_${deterministicId(seed, 12)}`;
}

export function generateReferralLinkId(
  userId: string,
  cardName: string
): string {
  const seed = `referral_link:${userId}:${cardName}`;
  return deterministicId(seed, 16);
}

export function generateAgentDiscoverableToolId(toolName: string): string {
  const seed = `agent_discoverable_tool:${toolName}`;
  return deterministicId(seed, 16);
}

export function generateCreditCardOrderId(
  creditCardAccountId: string,
  userId: string,
  reason: string
): string {
  const seed = `credit_card_order:${creditCardAccountId}:${userId}:${reason}`;
  return `ccord_${deterministicId(seed, 12)}`;
}

export function generateClosureReasonId(
  creditCardAccountId: string,
  userId: string
): string {
  const seed = `closure_reason:${creditCardAccountId}:${userId}`;
  return `clsr_${deterministicId(seed, 12)}`;
}

export function generateAccountFlagId(
  creditCardAccountId: string,
  flagType: string,
  expirationDate: string
): string {
  const seed = `account_flag:${creditCardAccountId}:${flagType}:${expirationDate}`;
  return `ccflag_${deterministicId(seed, 12)}`;
}

export function generateCreditLimitIncreaseRequestId(
  creditCardAccountId: string,
  userId: string,
  requestedIncreaseAmount: number
): string {
  const seed = `cli_request:${creditCardAccountId}:${userId}:${requestedIncreaseAmount.toFixed(2)}`;
  return `cli_${deterministicId(seed, 12)}`;
}

export function generateBankAccountTransactionId(opts: {
  accountId: string;
  date: string;
  description: string;
  amount: number;
  transactionType: string;
}): string {
  const { accountId, date, description, amount, transactionType } = opts;
  const seed = `bank_txn:${accountId}:${date}:${description}:${amount.toFixed(2)}:${transactionType}`;
  return `btxn_${deterministicId(seed, 12)}`;
}

export function generateDebitCardOrderId(
  accountId: string,
  userId: string,
  deliveryOption: string
): string {
  const seed = `debit_card_order:${accountId}:${userId}:${deliveryOption}`;
  return `dcord_${deterministicId(seed, 12)}`;
}

export function generateDebitCardId(
  accountId: string,
  userId: string,
  issueDate: string
): string {
  const seed = `debit_card:${accountId}:${userId}:${issueDate}`;
  return `dbc_${deterministicId(seed, 12)}`;
}
