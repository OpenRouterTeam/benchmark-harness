import { createHash } from "node:crypto";

import { addToDb, queryDatabaseTool, queryDb } from "./db-query";
import {
  getFieldAsString,
  getFieldAsBoolean,
  getFieldCoalesced,
} from "./field-access";
import { getAccountBalance, getTodayStr } from "./helpers";
import type { BankingEnvState } from "./registry";
import { registerDiscoverableAgentTool } from "./registry";

function toFiniteNumber(value: unknown): number | undefined {
  if (typeof value === "number") {
    return Number.isFinite(value) ? value : undefined;
  }
  if (typeof value === "string") {
    const parsed = Number.parseFloat(value);
    return Number.isFinite(parsed) ? parsed : undefined;
  }
  return undefined;
}

function getNow(): Date {
  return new Date(2025, 10, 14, 3, 40, 0);
}

function getAccountAgeDays(account: Record<string, unknown>): number {
  const dateOpenedStr = getFieldAsString(account, "date_opened");
  if (!dateOpenedStr) {
    return 0;
  }
  try {
    const parts = dateOpenedStr.split("/");
    if (parts.length !== 3) {
      return 0;
    }
    const month = Number.parseInt(parts[0]!, 10);
    const day = Number.parseInt(parts[1]!, 10);
    const year = Number.parseInt(parts[2]!, 10);
    const dateOpened = new Date(year, month - 1, day);
    const today = getNow();
    const diffTime = today.getTime() - dateOpened.getTime();
    return Math.floor(diffTime / (1000 * 60 * 60 * 24));
  } catch {
    return 0;
  }
}

function openBankAccount(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = getFieldAsString(kwargs, "user_id");
  const accountType = getFieldAsString(kwargs, "account_type");
  const accountClass = getFieldAsString(kwargs, "account_class");
  if (!userId || !accountType || !accountClass) {
    return "Error: Missing required parameters.";
  }
  const validTypes = [
    "checking",
    "savings",
    "business_checking",
    "business_savings",
  ];
  if (!validTypes.includes(accountType)) {
    return `Error: Invalid account_type. Must be one of: ${JSON.stringify(validTypes)}`;
  }
  const userAccounts = queryDb({
    db: state.db,
    dbName: "accounts",
    constraints: { user_id: userId },
  });
  if (accountType === "savings") {
    let hasEligibleChecking = false;
    for (const acc of userAccounts) {
      const accType = getFieldCoalesced(acc, "account_type", "class");
      const { status } = acc;
      if (
        (accType === "checking" || accType === "personal_checking") &&
        (status === "OPEN" || status === "ACTIVE") &&
        getAccountAgeDays(acc) >= 14
      ) {
        hasEligibleChecking = true;
        break;
      }
    }
    if (!hasEligibleChecking) {
      return "Error: Account eligibility requirements not met.";
    }
  }
  if (accountType === "business_checking") {
    const hasClosedAccount = userAccounts.some(
      (acc) => acc.status === "CLOSED"
    );
    if (hasClosedAccount) {
      return "Error: Account eligibility requirements not met.";
    }
    const hasPersonalChecking = userAccounts.some((acc) => {
      const accType = getFieldCoalesced(acc, "account_type", "class");
      const { status } = acc;
      return (
        (accType === "checking" || accType === "personal_checking") &&
        (status === "OPEN" || status === "ACTIVE")
      );
    });
    if (!hasPersonalChecking) {
      return "Error: Account eligibility requirements not met.";
    }
  }
  if (accountType === "business_savings") {
    for (const acc of userAccounts) {
      const balance = getAccountBalance(acc);
      if (balance < 0) {
        return "Error: Account eligibility requirements not met.";
      }
    }
    let hasEligibleBusinessChecking = false;
    for (const acc of userAccounts) {
      const accType = acc.account_type;
      const { status } = acc;
      if (
        accType === "business_checking" &&
        (status === "OPEN" || status === "ACTIVE") &&
        getAccountAgeDays(acc) >= 30
      ) {
        hasEligibleBusinessChecking = true;
        break;
      }
    }
    if (!hasEligibleBusinessChecking) {
      return "Error: Account eligibility requirements not met.";
    }
  }
  const seed = `account:${userId}:${accountType}:${accountClass}`;
  const accountId = createHash("sha256")
    .update(seed)
    .digest()
    .slice(0, 8)
    .toString("hex");
  const today = getTodayStr();
  const accountRecord: Record<string, unknown> = {
    account_id: accountId,
    user_id: userId,
    account_type: accountType,
    account_class: accountClass,
    current_holdings: "0.00",
    status: "OPEN",
    date_opened: today,
  };
  const success = addToDb({
    db: state.db,
    dbName: "accounts",
    recordId: accountId,
    record: accountRecord,
  });
  if (!success) {
    return `Failed to open account: Account ID '${accountId}' may already exist.`;
  }
  return (
    `Bank account opened successfully!\n` +
    `  - Account ID: ${accountId}\n` +
    `  - User ID: ${userId}\n` +
    `  - Account Type: ${accountType}\n` +
    `  - Account Class: ${accountClass}\n` +
    `  - Status: OPEN\n` +
    `  - Initial Balance: $0.00\n` +
    `  - Date Opened: ${today}`
  );
}

function closeBankAccount(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const accountId = getFieldAsString(kwargs, "account_id");
  const reason =
    getFieldAsString(kwargs, "reason") ?? "Customer requested closure";
  const waiveEarlyClosureFee = getFieldAsBoolean(
    kwargs,
    "waive_early_closure_fee"
  );
  const PERSONAL_CHECKING_EARLY_CLOSURE: Record<
    string,
    {
      fee: number;
      window_days: number;
    }
  > = {
    "Light Blue Account": { fee: 15, window_days: 30 },
    "Light Green Account": { fee: 15, window_days: 30 },
    "Green Fee-Free Account": { fee: 15, window_days: 30 },
    "Blue Account": { fee: 25, window_days: 60 },
    "Green Account": { fee: 25, window_days: 60 },
    "Evergreen Account": { fee: 50, window_days: 90 },
    "Bluest Account": { fee: 100, window_days: 180 },
  };
  const PERSONAL_SAVINGS_EARLY_CLOSURE: Record<
    string,
    {
      fee: number;
      window_days: number;
    }
  > = {
    "Bronze Account": { fee: 20, window_days: 60 },
    "Silver Account": { fee: 35, window_days: 90 },
    "Silver Plus Account": { fee: 35, window_days: 90 },
    "Gold Account": { fee: 75, window_days: 180 },
    "Gold Plus Account": { fee: 75, window_days: 180 },
    "Gold Years Account": { fee: 75, window_days: 180 },
    "Platinum Account": { fee: 150, window_days: 270 },
    "Platinum Plus Account": { fee: 150, window_days: 270 },
    "Diamond Elite Account": { fee: 150, window_days: 270 },
  };
  if (!accountId) {
    return "Error: Missing required parameter (account_id).";
  }
  if (!(accountId in state.db.accounts.data)) {
    return `Error: Account '${accountId}' not found.`;
  }
  const account = state.db.accounts.data[accountId];
  if (!account || typeof account !== "object") {
    return `Error: Account '${accountId}' not found.`;
  }
  if (account.status === "CLOSED") {
    return `Error: Account '${accountId}' is already closed.`;
  }
  const balance = getAccountBalance(account);
  let earlyClosureFeeApplied = 0;
  if (!waiveEarlyClosureFee) {
    const accountLevel = getFieldAsString(account, "level");
    const accountClass = getFieldAsString(account, "class");
    const dateOpenedStr = getFieldAsString(account, "date_opened");
    let earlyClosureConfig:
      | {
          fee: number;
          window_days: number;
        }
      | undefined;
    if (accountClass === "checking" && accountLevel) {
      earlyClosureConfig = PERSONAL_CHECKING_EARLY_CLOSURE[accountLevel];
    } else if (
      (accountClass === "savings" || accountClass === "saving") &&
      accountLevel
    ) {
      earlyClosureConfig = PERSONAL_SAVINGS_EARLY_CLOSURE[accountLevel];
    }
    if (earlyClosureConfig && dateOpenedStr) {
      try {
        const accountAgeDays = getAccountAgeDays(account);
        if (accountAgeDays < earlyClosureConfig.window_days) {
          const requiredFee = earlyClosureConfig.fee;
          if (balance < requiredFee) {
            return "Error: Account unable to be closed.";
          }
          earlyClosureFeeApplied = requiredFee;
        }
      } catch {}
    }
  }
  const remainingBalance = balance - earlyClosureFeeApplied;
  if (remainingBalance !== 0) {
    return `Error: Account balance must be $0.00 before closing. Current balance: $${balance.toFixed(2)}`;
  }
  const today = getTodayStr();
  account.status = "CLOSED";
  account.date_closed = today;
  account.closure_reason = reason;
  account.early_closure_fee_waived = waiveEarlyClosureFee;
  return (
    `Bank account closed successfully!\n` +
    `  - Account ID: ${accountId}\n` +
    `  - Account Type: ${getFieldAsString(account, "account_type") ?? "N/A"}\n` +
    `  - Account Class: ${getFieldAsString(account, "account_class") ?? "N/A"}\n` +
    `  - Status: CLOSED\n` +
    `  - Date Closed: ${today}\n` +
    `  - Reason: ${reason}\n` +
    `  - Early Closure Fee Waived: ${waiveEarlyClosureFee ? "Yes" : "No"}`
  );
}

function getAllUserAccountsByUserId(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = getFieldAsString(kwargs, "user_id");
  if (!userId) {
    return "Error: Missing required parameter: user_id";
  }
  const accountsResult = queryDatabaseTool(
    state.db,
    "accounts",
    JSON.stringify({ user_id: userId })
  );
  const ccResult = queryDatabaseTool(
    state.db,
    "credit_card_accounts",
    JSON.stringify({ user_id: userId })
  );
  const resultParts = [
    "User accounts retrieved successfully.",
    "",
    `Executed: get_all_user_accounts_by_user_id_3847`,
    `Accounts for user ${userId}:`,
    "",
    "Bank Accounts:",
  ];
  if (
    !accountsResult.includes("No records found") &&
    !accountsResult.includes("No results found")
  ) {
    resultParts.push(accountsResult);
  } else {
    resultParts.push("  No bank accounts found.");
  }
  resultParts.push("\nCredit Card Accounts:");
  if (
    !ccResult.includes("No records found") &&
    !ccResult.includes("No results found")
  ) {
    resultParts.push(ccResult);
  } else {
    resultParts.push("  No credit card accounts found.");
  }
  return resultParts.join("\n");
}

function transferFundsBetweenBankAccounts(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const sourceAccountId = getFieldAsString(kwargs, "source_account_id");
  const destinationAccountId = getFieldAsString(
    kwargs,
    "destination_account_id"
  );
  const { amount } = kwargs;
  if (
    !sourceAccountId ||
    !destinationAccountId ||
    amount === null ||
    amount === undefined
  ) {
    return "Error: Missing required parameters (source_account_id, destination_account_id, amount).";
  }
  let parsedAmount: number;
  try {
    parsedAmount =
      typeof amount === "string"
        ? Number.parseFloat(amount)
        : (amount as number);
  } catch {
    return `Error: Invalid amount '${amount}'. Must be a number.`;
  }
  if (isNaN(parsedAmount)) {
    return `Error: Invalid amount '${amount}'. Must be a number.`;
  }
  if (parsedAmount <= 0) {
    return "Error: Transfer amount must be positive.";
  }
  if (sourceAccountId === destinationAccountId) {
    return "Error: Source and destination accounts cannot be the same.";
  }
  if (!(sourceAccountId in state.db.accounts.data)) {
    return `Error: Source account '${sourceAccountId}' not found.`;
  }
  if (!(destinationAccountId in state.db.accounts.data)) {
    return `Error: Destination account '${destinationAccountId}' not found.`;
  }
  const source = state.db.accounts.data[sourceAccountId];
  if (!source || typeof source !== "object") {
    return `Error: Source account '${sourceAccountId}' not found.`;
  }
  if (!(source.status === "ACTIVE" || source.status === "OPEN")) {
    return `Error: Source account '${sourceAccountId}' is not active.`;
  }
  const dest = state.db.accounts.data[destinationAccountId];
  if (!dest || typeof dest !== "object") {
    return `Error: Destination account '${destinationAccountId}' not found.`;
  }
  if (!(dest.status === "ACTIVE" || dest.status === "OPEN")) {
    return `Error: Destination account '${destinationAccountId}' is not active.`;
  }
  const sourceBalance = getAccountBalance(source);
  if (sourceBalance < parsedAmount) {
    return (
      `Error: Insufficient funds. Source account balance is $${sourceBalance.toFixed(2)}, ` +
      `but transfer amount is $${parsedAmount.toFixed(2)}.`
    );
  }
  const destBalance = getAccountBalance(dest);
  const newSource = sourceBalance - parsedAmount;
  const newDest = destBalance + parsedAmount;
  source.current_holdings = `$${newSource.toFixed(2)}`;
  dest.current_holdings = `$${newDest.toFixed(2)}`;
  return (
    `Transfer completed successfully!\n` +
    `  - Amount: $${parsedAmount.toFixed(2)}\n` +
    `  - From: ${sourceAccountId} (new balance: $${newSource.toFixed(2)})\n` +
    `  - To: ${destinationAccountId} (new balance: $${newDest.toFixed(2)})`
  );
}

function applyCheckingAccountCredit(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const accountId = getFieldAsString(kwargs, "account_id");
  const { amount } = kwargs;
  const creditType = getFieldAsString(kwargs, "credit_type");
  if (!accountId || amount === null || amount === undefined || !creditType) {
    return "Error: Missing required parameters.";
  }
  const parsedAmount =
    typeof amount === "number" ? amount : Number(String(amount));
  if (!Number.isFinite(parsedAmount)) {
    return "Error: Invalid credit amount. Must be a number.";
  }
  if (parsedAmount <= 0) {
    return "Error: Credit amount must be positive.";
  }
  const validTypes = ["rebate_credit", "fee_refund"];
  if (!validTypes.includes(creditType)) {
    return `Error: Invalid credit_type. Must be one of: ${validTypes}`;
  }
  if (!(accountId in state.db.accounts.data)) {
    return `Error: Account '${accountId}' not found.`;
  }
  const account = state.db.accounts.data[accountId];
  if (!account || typeof account !== "object") {
    return `Error: Account '${accountId}' not found.`;
  }
  const accountClass = getFieldAsString(account, "class");
  if (!accountClass || accountClass.toLowerCase() !== "checking") {
    return `Error: Account '${accountId}' is not a checking account. Credits can only be applied to checking accounts.`;
  }
  if (!(account.status === "ACTIVE" || account.status === "OPEN")) {
    return `Error: Account '${accountId}' is not active.`;
  }
  const currentBalance = getAccountBalance(account);
  const newBalance = currentBalance + parsedAmount;
  account.current_holdings = `$${newBalance.toFixed(2)}`;
  const seed = `checking_credit:${accountId}:${creditType}:${parsedAmount}:${getTodayStr()}`;
  const txnHash = createHash("sha256")
    .update(seed)
    .digest()
    .slice(0, 6)
    .toString("hex");
  const transactionId = `txn_${txnHash}`;
  const description =
    creditType === "rebate_credit"
      ? "REBATE CREDIT - CUSTOMER SERVICE"
      : "FEE REFUND - CUSTOMER SERVICE";
  const transactionRecord: Record<string, unknown> = {
    transaction_id: transactionId,
    account_id: accountId,
    date: getTodayStr(),
    description,
    amount: parsedAmount,
    type: creditType,
    status: "posted",
  };
  state.db.bank_account_transaction_history.data[transactionId] =
    transactionRecord;
  return (
    `\nCredit applied successfully!\n` +
    `  - Transaction ID: ${transactionId}\n` +
    `  - Account: ${accountId}\n` +
    `  - Credit Type: ${creditType}\n` +
    `  - Amount: $${parsedAmount.toFixed(2)}\n` +
    `  - Previous Balance: $${currentBalance.toFixed(2)}\n` +
    `  - New Balance: $${newBalance.toFixed(2)}`
  );
}

function applySavingsAccountCredit(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const accountId = getFieldAsString(kwargs, "account_id");
  const { amount } = kwargs;
  const creditType = getFieldAsString(kwargs, "credit_type");
  if (!accountId || amount === null || amount === undefined || !creditType) {
    return "Error: Missing required parameters.";
  }
  const parsedAmount =
    typeof amount === "number" ? amount : Number(String(amount));
  if (!Number.isFinite(parsedAmount)) {
    return "Error: Invalid credit amount. Must be a number.";
  }
  if (parsedAmount <= 0) {
    return "Error: Credit amount must be positive.";
  }
  const validTypes = ["interest_correction", "fee_refund", "goodwill_credit"];
  if (!validTypes.includes(creditType)) {
    return `Error: Invalid credit_type. Must be one of: ${validTypes}`;
  }
  if (!(accountId in state.db.accounts.data)) {
    return `Error: Account '${accountId}' not found.`;
  }
  const account = state.db.accounts.data[accountId];
  if (!account || typeof account !== "object") {
    return `Error: Account '${accountId}' not found.`;
  }
  const accountClass = getFieldAsString(account, "class");
  if (
    !accountClass ||
    (accountClass.toLowerCase() !== "saving" &&
      accountClass.toLowerCase() !== "savings")
  ) {
    return `Error: Account '${accountId}' is not a savings account. This tool only applies to savings accounts.`;
  }
  if (!(account.status === "ACTIVE" || account.status === "OPEN")) {
    return `Error: Account '${accountId}' is not active.`;
  }
  const currentBalance = getAccountBalance(account);
  const newBalance = currentBalance + parsedAmount;
  account.current_holdings = `${newBalance.toFixed(2)}`;
  const seed = `savings_credit:${accountId}:${creditType}:${parsedAmount}:${getTodayStr()}`;
  const txnHash = createHash("sha256")
    .update(seed)
    .digest()
    .slice(0, 6)
    .toString("hex");
  const transactionId = `txn_${txnHash}`;
  let description: string;
  if (creditType === "interest_correction") {
    description = "INTEREST CORRECTION - CUSTOMER SERVICE";
  } else if (creditType === "fee_refund") {
    description = "FEE REFUND - CUSTOMER SERVICE";
  } else {
    description = "GOODWILL CREDIT - CUSTOMER SERVICE";
  }
  const transactionRecord: Record<string, unknown> = {
    transaction_id: transactionId,
    account_id: accountId,
    date: getTodayStr(),
    description,
    amount: parsedAmount,
    type: creditType,
    status: "posted",
  };
  state.db.bank_account_transaction_history.data[transactionId] =
    transactionRecord;
  return (
    `\nCredit applied successfully!\n` +
    `  - Transaction ID: ${transactionId}\n` +
    `  - Account: ${accountId}\n` +
    `  - Credit Type: ${creditType}\n` +
    `  - Amount: $${parsedAmount.toFixed(2)}\n` +
    `  - Previous Balance: $${currentBalance.toFixed(2)}\n` +
    `  - New Balance: $${newBalance.toFixed(2)}`
  );
}

function submitInterestDiscrepancyReport(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const accountId = getFieldAsString(kwargs, "account_id");
  const userId = getFieldAsString(kwargs, "user_id");
  const expectedApy = kwargs.expected_apy;
  const actualApy = kwargs.actual_apy;
  const amountDifference = kwargs.amount_difference;
  if (
    !accountId ||
    !userId ||
    expectedApy === null ||
    expectedApy === undefined ||
    actualApy === null ||
    actualApy === undefined ||
    amountDifference === null ||
    amountDifference === undefined
  ) {
    return "Error: Missing required parameters.";
  }
  const parsedExpectedApy = toFiniteNumber(expectedApy);
  const parsedActualApy = toFiniteNumber(actualApy);
  const parsedAmountDifference = toFiniteNumber(amountDifference);
  if (
    parsedExpectedApy === undefined ||
    parsedActualApy === undefined ||
    parsedAmountDifference === undefined
  ) {
    return "Error: expected_apy, actual_apy, and amount_difference must be numbers.";
  }
  const account = state.db.accounts.data[accountId];
  if (!account || typeof account !== "object") {
    return `Error: Account '${accountId}' not found.`;
  }
  const user = state.db.users.data[userId];
  if (!user || typeof user !== "object") {
    return `Error: User '${userId}' not found.`;
  }
  const seed = `interest_report:${accountId}:${userId}:${parsedExpectedApy}:${parsedActualApy}:${getTodayStr()}`;
  const reportHash = createHash("sha256")
    .update(seed)
    .digest()
    .slice(0, 6)
    .toString("hex");
  const reportId = `IDR_${reportHash}`;
  const apyDifference =
    Math.round((parsedExpectedApy - parsedActualApy) * 10000) / 10000;
  const reportRecord: Record<string, unknown> = {
    report_id: reportId,
    account_id: accountId,
    user_id: userId,
    account_level: getFieldAsString(account, "level") ?? "Unknown",
    expected_apy: parsedExpectedApy,
    actual_apy: parsedActualApy,
    apy_difference: apyDifference,
    amount_difference: parsedAmountDifference,
    submitted_date: getTodayStr(),
    status: "PENDING_REVIEW",
  };
  addToDb({
    db: state.db,
    dbName: "interest_discrepancy_reports",
    recordId: reportId,
    record: reportRecord,
  });
  return (
    `\nInterest Discrepancy Report Submitted Successfully!\n` +
    `  - Report ID: ${reportId}\n` +
    `  - Account: ${accountId} (${getFieldAsString(account, "level") ?? "Unknown"})\n` +
    `  - Customer: ${getFieldAsString(user, "name") ?? "Unknown"}\n` +
    `  - Expected APY: ${parsedExpectedApy}%\n` +
    `  - Actual APY: ${parsedActualApy}%\n` +
    `  - APY Difference: ${apyDifference}%\n` +
    `  - Amount Difference: $${parsedAmountDifference.toFixed(2)}\n` +
    `  - Status: PENDING_REVIEW\n` +
    `\nThe backend team will investigate this discrepancy and ensure ` +
    `correct APY calculations are applied going forward.`
  );
}

function getBankAccountTransactions(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const accountId = getFieldAsString(kwargs, "account_id");
  if (!accountId) {
    return "Error: Missing required parameter: account_id";
  }
  if (!(accountId in state.db.accounts.data)) {
    return `Error: Account '${accountId}' not found.`;
  }
  const resultParts = [
    "Bank account transactions retrieved successfully.",
    "",
    `Executed: get_bank_account_transactions_9173`,
    `Transactions for account ${accountId}:`,
  ];
  const txns = queryDb({
    db: state.db,
    dbName: "bank_account_transaction_history",
    constraints: { account_id: accountId },
    options: { returnIds: true },
  });
  if (txns.length > 0) {
    const parseDate = (value: unknown): number => {
      const dateString = String(value ?? "");
      for (const format of ["datetime", "date"] as const) {
        const match =
          format === "datetime"
            ? /^(\d{2})\/(\d{2})\/(\d{4}) (\d{2}):(\d{2}):(\d{2})$/u.exec(
                dateString
              )
            : /^(\d{2})\/(\d{2})\/(\d{4})$/u.exec(dateString);
        if (match) {
          const [
            ,
            month,
            day,
            year,
            hour = "00",
            minute = "00",
            second = "00",
          ] = match;
          const timestamp = Date.UTC(
            Number(year),
            Number(month) - 1,
            Number(day),
            Number(hour),
            Number(minute),
            Number(second)
          );
          const parsed = new Date(timestamp);
          if (
            parsed.getUTCFullYear() === Number(year) &&
            parsed.getUTCMonth() === Number(month) - 1 &&
            parsed.getUTCDate() === Number(day) &&
            parsed.getUTCHours() === Number(hour) &&
            parsed.getUTCMinutes() === Number(minute) &&
            parsed.getUTCSeconds() === Number(second)
          ) {
            return timestamp;
          }
        }
      }
      return Number.MIN_SAFE_INTEGER;
    };
    const sortedTxns = [...txns].sort(
      ([, left], [, right]) => parseDate(right.date) - parseDate(left.date)
    );
    const formattedLines = [
      `Found ${sortedTxns.length} record(s) in 'bank_account_transaction_history':\n`,
    ];
    for (const [index, [recordId, record]] of sortedTxns.entries()) {
      formattedLines.push(`${index + 1}. Record ID: ${recordId}`);
      for (const [field, value] of Object.entries(record)) {
        formattedLines.push(`   ${field}: ${value}`);
      }
      formattedLines.push("");
    }
    resultParts.push(formattedLines.join("\n"));
  } else {
    resultParts.push("\nNo transactions found for this account.");
  }
  return resultParts.join("\n");
}

export function registerAccountTools(): void {
  registerDiscoverableAgentTool({
    name: "open_bank_account_4821",
    description: "Open a new bank account for a customer.",
    params: [
      {
        name: "user_id",
        type: "string",
        optional: false,
        description: "The customer's unique identifier in the system",
      },
      {
        name: "account_type",
        type: "string",
        optional: false,
        description:
          "Type of account to open. Must be one of: 'checking' (personal checking), 'savings' (personal savings), 'business_checking', 'business_savings'",
      },
      {
        name: "account_class",
        type: "string",
        optional: false,
        description: "The full official account class name",
      },
    ],
    handler: openBankAccount,
  });
  registerDiscoverableAgentTool({
    name: "close_bank_account_7392",
    description: "Close a customer's bank account (checking or savings).",
    params: [
      {
        name: "account_id",
        type: "string",
        optional: false,
        description: "The ID of the bank account to close",
      },
      {
        name: "reason",
        type: "string",
        optional: true,
        description: "The reason for closing the account",
      },
      {
        name: "waive_early_closure_fee",
        type: "boolean",
        optional: true,
        description: "Whether to waive early closure fees",
      },
    ],
    handler: closeBankAccount,
  });
  registerDiscoverableAgentTool({
    name: "get_all_user_accounts_by_user_id_3847",
    mutatesState: false,
    description:
      "Retrieve all accounts (checking, savings, credit cards) for a customer.",
    params: [
      {
        name: "user_id",
        type: "string",
        optional: false,
        description: "The customer's unique identifier in the system",
      },
    ],
    handler: getAllUserAccountsByUserId,
  });
  registerDiscoverableAgentTool({
    name: "transfer_funds_between_bank_accounts_7291",
    description: "Transfer funds from one bank account to another.",
    params: [
      {
        name: "source_account_id",
        type: "string",
        optional: false,
        description: "The account ID to transfer funds from",
      },
      {
        name: "destination_account_id",
        type: "string",
        optional: false,
        description: "The account ID to transfer funds to",
      },
      {
        name: "amount",
        type: "number",
        optional: false,
        description: "The amount to transfer in USD",
      },
    ],
    handler: transferFundsBetweenBankAccounts,
  });
  registerDiscoverableAgentTool({
    name: "apply_checking_account_credit_5829",
    description: "Apply a credit to a customer's checking account.",
    params: [
      {
        name: "account_id",
        type: "string",
        optional: false,
        description: "The checking account ID to credit",
      },
      {
        name: "amount",
        type: "number",
        optional: false,
        description:
          "The positive dollar amount to credit (must be greater than 0)",
      },
      {
        name: "credit_type",
        type: "string",
        optional: false,
        description:
          "The type of credit: 'rebate_credit' for missing rebates, 'fee_refund' for incorrect fee charges",
      },
    ],
    handler: applyCheckingAccountCredit,
  });
  registerDiscoverableAgentTool({
    name: "apply_savings_account_credit_6831",
    description:
      "Apply a credit to a customer's savings account for interest corrections, fee refunds, or goodwill adjustments.",
    params: [
      {
        name: "account_id",
        type: "string",
        optional: false,
        description: "The savings account ID to credit",
      },
      {
        name: "amount",
        type: "number",
        optional: false,
        description:
          "The positive dollar amount to credit (must be greater than 0)",
      },
      {
        name: "credit_type",
        type: "string",
        optional: false,
        description:
          "The type of credit: 'interest_correction' for APY/interest calculation errors, 'fee_refund' for incorrect fee charges, 'goodwill_credit' for customer service gestures",
      },
    ],
    handler: applySavingsAccountCredit,
  });
  registerDiscoverableAgentTool({
    name: "submit_interest_discrepancy_report_7294",
    description:
      "Submit a report for interest calculation discrepancies to the backend team for investigation. Use this when the interest credited to a customer's account does not match expected APY calculations.",
    params: [
      {
        name: "account_id",
        type: "string",
        optional: false,
        description: "The savings account ID with the discrepancy",
      },
      {
        name: "user_id",
        type: "string",
        optional: false,
        description: "The customer's unique identifier",
      },
      {
        name: "expected_apy",
        type: "number",
        optional: false,
        description:
          "The APY percentage the customer should have received (e.g., 2.775 for 2.775%)",
      },
      {
        name: "actual_apy",
        type: "number",
        optional: false,
        description:
          "The APY percentage that was actually applied (e.g., 2.5 for 2.5%)",
      },
      {
        name: "amount_difference",
        type: "number",
        optional: false,
        description:
          "The dollar amount difference between expected and actual interest credited",
      },
    ],
    handler: submitInterestDiscrepancyReport,
  });
  registerDiscoverableAgentTool({
    name: "get_bank_account_transactions_9173",
    mutatesState: false,
    description: "Retrieve the transaction history for a bank account.",
    params: [
      {
        name: "account_id",
        type: "string",
        optional: false,
        description: "The bank account ID to retrieve transactions for",
      },
    ],
    handler: getBankAccountTransactions,
  });
}
