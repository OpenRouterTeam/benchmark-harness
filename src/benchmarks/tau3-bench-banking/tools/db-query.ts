import { Either } from "../../../internal/either";
import { unknownErrorToString } from "../../../internal/errors";
import type { BankingData } from "../types";
import { getBankingTable } from "../types";

type BankingDb = BankingData;

type ComparisonOperator =
  | "eq"
  | "ne"
  | "gt"
  | "gte"
  | "lt"
  | "lte"
  | "contains"
  | "startswith"
  | "endswith"
  | "in"
  | "nin";

interface QueryOptions {
  returnIds?: boolean;
  limit?: number;
}

function setOwnProperty(
  target: Record<string, unknown>,
  key: string,
  value: unknown
): void {
  Object.defineProperty(target, key, {
    value,
    writable: true,
    enumerable: true,
    configurable: true,
  });
}

function isComparisonOperator(op: string): op is ComparisonOperator {
  return [
    "eq",
    "ne",
    "gt",
    "gte",
    "lt",
    "lte",
    "contains",
    "startswith",
    "endswith",
    "in",
    "nin",
  ].includes(op);
}

function parseConstraint(
  key: string,
  value: unknown
): [string, ComparisonOperator, unknown] {
  if (key.includes("__")) {
    const parts = key.split("__", 2);
    const fieldName = parts[0]!;
    const opName = parts[1]!;
    if (!isComparisonOperator(opName)) {
      return [key, "eq", value];
    }
    return [fieldName, opName, value];
  }
  return [key, "eq", value];
}

function getComparisonOp(
  opName: ComparisonOperator
): (a: unknown, b: unknown) => boolean {
  switch (opName) {
    case "eq": {
      return (a, b) => a === b;
    }
    case "ne": {
      return (a, b) => a !== b;
    }
    case "gt": {
      return (a, b) => {
        if (
          (typeof a === "number" && typeof b === "number") ||
          (typeof a === "string" && typeof b === "string")
        ) {
          return a > b;
        }
        return false;
      };
    }
    case "gte": {
      return (a, b) => {
        if (
          (typeof a === "number" && typeof b === "number") ||
          (typeof a === "string" && typeof b === "string")
        ) {
          return a >= b;
        }
        return false;
      };
    }
    case "lt": {
      return (a, b) => {
        if (
          (typeof a === "number" && typeof b === "number") ||
          (typeof a === "string" && typeof b === "string")
        ) {
          return a < b;
        }
        return false;
      };
    }
    case "lte": {
      return (a, b) => {
        if (
          (typeof a === "number" && typeof b === "number") ||
          (typeof a === "string" && typeof b === "string")
        ) {
          return a <= b;
        }
        return false;
      };
    }
    case "contains": {
      return (a, b) => {
        if (a === null || a === undefined) {
          return false;
        }
        if (Array.isArray(a)) {
          return a.includes(b);
        }
        if (typeof a === "string" && typeof b === "string") {
          return a.includes(b);
        }
        return false;
      };
    }
    case "startswith": {
      return (a, b) => {
        if (a === null || a === undefined) {
          return false;
        }
        return String(a).startsWith(String(b));
      };
    }
    case "endswith": {
      return (a, b) => {
        if (a === null || a === undefined) {
          return false;
        }
        return String(a).endsWith(String(b));
      };
    }
    case "in": {
      return (a, b) => {
        if (Array.isArray(b)) {
          return b.includes(a);
        }
        return false;
      };
    }
    case "nin": {
      return (a, b) => {
        if (Array.isArray(b)) {
          return !b.includes(a);
        }
        return false;
      };
    }
    default: {
      return (a, b) => a === b;
    }
  }
}

function recordMatches(
  record: Record<string, unknown>,
  constraints: Record<string, unknown>
): boolean {
  for (const [key, value] of Object.entries(constraints)) {
    const [fieldName, opName, expected] = parseConstraint(key, value);
    const actual = record[fieldName];
    const compare = getComparisonOp(opName);
    try {
      if (!compare(actual, expected)) {
        return false;
      }
    } catch {
      return false;
    }
  }
  return true;
}

export function queryDb(opts: {
  db: BankingDb;
  dbName: string;
  constraints?: Record<string, unknown>;
  options: QueryOptions & {
    returnIds: true;
  };
}): [string, Record<string, unknown>][];

export function queryDb(opts: {
  db: BankingDb;
  dbName: string;
  constraints?: Record<string, unknown>;
  options?: QueryOptions;
}): Record<string, unknown>[];

export function queryDb(opts: {
  db: BankingDb;
  dbName: string;
  constraints?: Record<string, unknown>;
  options?: QueryOptions;
}): Record<string, unknown>[] | [string, Record<string, unknown>][] {
  const { db, dbName, constraints, options } = opts;
  const finalConstraints = constraints ?? {};
  const { returnIds = false, limit } = options ?? {};
  const table = getBankingTable(db, dbName);
  if (!table) {
    return [];
  }
  const resultsWithIds: [string, Record<string, unknown>][] = [];
  const resultsNoIds: Record<string, unknown>[] = [];
  for (const [recordId, record] of Object.entries(table.data)) {
    if (!record || typeof record !== "object") {
      continue;
    }
    const recordObj = record satisfies Record<string, unknown>;
    if (recordMatches(recordObj, finalConstraints)) {
      if (returnIds) {
        resultsWithIds.push([recordId, recordObj]);
      } else {
        resultsNoIds.push(recordObj);
      }
      const currentLength = returnIds
        ? resultsWithIds.length
        : resultsNoIds.length;
      if (limit !== undefined && currentLength >= limit) {
        break;
      }
    }
  }
  return returnIds ? resultsWithIds : resultsNoIds;
}

export function addToDb(opts: {
  db: BankingDb;
  dbName: string;
  recordId: string;
  record: Record<string, unknown>;
}): boolean {
  const { db, dbName, recordId, record } = opts;
  const table = getBankingTable(db, dbName);
  if (!table) {
    return false;
  }
  if (Object.hasOwn(table.data, recordId)) {
    return false;
  }
  setOwnProperty(table.data, recordId, record);
  return true;
}

export function updateRecordInDb(opts: {
  db: BankingDb;
  dbName: string;
  recordId: string;
  updates: Record<string, unknown>;
}): [boolean, Record<string, unknown> | null] {
  const { db, dbName, recordId, updates } = opts;
  const table = getBankingTable(db, dbName);
  if (!table) {
    return [false, null];
  }
  if (!Object.hasOwn(table.data, recordId)) {
    return [false, null];
  }
  const record = table.data[recordId];
  if (typeof record !== "object" || record === null) {
    return [false, null];
  }
  const updated = record satisfies Record<string, unknown>;
  for (const [field, value] of Object.entries(updates)) {
    setOwnProperty(updated, field, value);
  }
  return [true, updated];
}

export function removeFromDb(opts: {
  db: BankingDb;
  dbName: string;
  constraints?: Record<string, unknown>;
}): Record<string, unknown>[] {
  const { db, dbName, constraints = {} } = opts;
  const table = getBankingTable(db, dbName);
  if (!table) {
    return [];
  }
  const results: Record<string, unknown>[] = [];
  const toDelete: string[] = [];
  for (const [recordId, record] of Object.entries(table.data)) {
    if (!record || typeof record !== "object") {
      continue;
    }
    const recordObj = record satisfies Record<string, unknown>;
    if (recordMatches(recordObj, constraints)) {
      results.push(recordObj);
      toDelete.push(recordId);
    }
  }
  for (const recordId of toDelete) {
    delete table.data[recordId];
  }
  return results;
}

export function listDatabases(db: BankingDb): string[] {
  const names = [
    "users",
    "accounts",
    "referrals",
    "credit_card_applications",
    "user_discoverable_tools",
    "user_discoverable_tool_calls",
    "agent_discoverable_tools",
    "task_config",
    "verification_history",
    "credit_card_transaction_history",
    "cash_back_disputes",
    "bank_account_transaction_history",
    "credit_card_accounts",
    "human_transfer_requests",
    "transaction_disputes",
    "credit_card_orders",
    "credit_card_closure_reasons",
    "credit_card_account_flags",
    "debit_cards",
    "debit_card_orders",
    "debit_card_disputes",
    "credit_limit_increase_requests",
    "payment_history",
  ];
  return names.filter((name) => name in db);
}

export function queryDatabaseTool(
  db: BankingDb,
  databaseName: string,
  constraints = "{}"
): string {
  const available = listDatabases(db);
  if (!available.includes(databaseName)) {
    return `Error: Database '${databaseName}' not found. Available: ${available.join(", ")}`;
  }
  let constraintDict: Record<string, unknown> = {};
  if (constraints) {
    const parsed = Either.try((): Record<string, unknown> =>
      JSON.parse(constraints)
    );
    if (Either.isLeft(parsed)) {
      return `Error: Invalid JSON: ${unknownErrorToString(parsed.left)}`;
    }
    constraintDict = parsed.right;
  }
  const results = queryDb({
    db,
    dbName: databaseName,
    constraints: constraintDict,
    options: { returnIds: true },
  });
  if (results.length === 0) {
    return `No records found in '${databaseName}'.`;
  }
  const lines: string[] = [
    `Found ${results.length} record(s) in '${databaseName}':\n`,
  ];
  for (let i = 0; i < results.length; i++) {
    const [recordId, record] = results[i] as [string, Record<string, unknown>];
    lines.push(`${i + 1}. Record ID: ${recordId}`);
    for (const [field, value] of Object.entries(record)) {
      lines.push(`   ${field}: ${value}`);
    }
    lines.push("");
  }
  return lines.join("\n");
}
