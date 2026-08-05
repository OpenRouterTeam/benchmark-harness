import { createHash } from "node:crypto";

import type { HttpClientError } from "@effect/platform";
import { HttpClient } from "@effect/platform";
import { TaggedError } from "effect/Data";
import type { Effect, Semaphore } from "effect/Effect";
import { fail, gen } from "effect/Effect";

import { Either } from "../../internal/either";
import { isDefinedAndNotNull, isRecord } from "../../internal/guards";
import { parseSchema } from "../../internal/zod";
import type { BankingData, BankingTable, Tau3Task } from "./types";
import { BANKING_TABLES, isBankingTableName, Tau3TaskSchema } from "./types";

export const BANKING_SOURCE_REVISION =
  "fc0055dc4e0a316c3f83133267fbd6faaa770992";

export const BANKING_SOURCE_BASE_URL = `https://raw.githubusercontent.com/sierra-research/tau2-bench/${BANKING_SOURCE_REVISION}/data/tau2/domains/banking_knowledge`;

let bankingDbCache: string | undefined;

let bankingTasksCache: string | undefined;

class FetchError extends TaggedError("FetchError")<{
  readonly message: string;
}> {}

function fetchGithubFile(
  filename: string
): Effect<
  string,
  FetchError | HttpClientError.HttpClientError,
  HttpClient.HttpClient
> {
  const url = `${BANKING_SOURCE_BASE_URL}/${filename}`;
  return gen(function* () {
    const client = yield* HttpClient.HttpClient;
    const response = yield* client.get(url);
    if (response.status < 200 || response.status >= 300) {
      return yield* fail(
        new FetchError({
          message: `Failed to fetch ${filename} from GitHub (${response.status})`,
        })
      );
    }
    return yield* response.text;
  });
}

export function ensureBankingData(
  fetchLock: Semaphore
): Effect<
  void,
  FetchError | HttpClientError.HttpClientError,
  HttpClient.HttpClient
> {
  return fetchLock.withPermits(1)(
    gen(function* () {
      if (bankingDbCache) {
        return;
      }
      bankingDbCache = yield* fetchGithubFile("db.json");
    })
  );
}

export function ensureBankingTasks(
  fetchLock: Semaphore
): Effect<
  void,
  FetchError | HttpClientError.HttpClientError,
  HttpClient.HttpClient
> {
  return fetchLock.withPermits(1)(
    gen(function* () {
      if (bankingTasksCache) {
        return;
      }
      bankingTasksCache = yield* fetchGithubFile("tasks.json");
    })
  );
}

export function seedBankingTasksRawCache(rawJson: string): void {
  bankingTasksCache = rawJson;
}

export function seedBankingCache(data: BankingData, tasks: Tau3Task[]): void {
  bankingDbCache = JSON.stringify(data);
  bankingTasksCache = JSON.stringify(tasks);
}

function emptyTable(): BankingTable {
  return { data: {}, notes: "" };
}

export function makeEmptyBankingData(): BankingData {
  return {
    users: emptyTable(),
    accounts: emptyTable(),
    debit_cards: emptyTable(),
    referrals: emptyTable(),
    credit_card_applications: emptyTable(),
    user_discoverable_tools: emptyTable(),
    user_discoverable_tool_calls: emptyTable(),
    verification_history: emptyTable(),
    credit_card_transaction_history: emptyTable(),
    cash_back_disputes: emptyTable(),
    bank_account_transaction_history: emptyTable(),
    credit_card_accounts: emptyTable(),
    agent_discoverable_tools: emptyTable(),
    task_config: emptyTable(),
    human_transfer_requests: emptyTable(),
    transaction_disputes: emptyTable(),
    credit_card_orders: emptyTable(),
    debit_card_orders: emptyTable(),
    credit_card_closure_reasons: emptyTable(),
    credit_card_account_flags: emptyTable(),
    credit_limit_increase_requests: emptyTable(),
    payment_history: emptyTable(),
    debit_card_disputes: emptyTable(),
  };
}

export function loadBankingData(): BankingData {
  const raw = bankingDbCache;
  if (!raw) {
    throw new Error("Banking data not loaded — call ensureBankingData() first");
  }
  const result = Either.try((): Record<string, unknown> => JSON.parse(raw));
  if (Either.isLeft(result)) {
    throw new Error("Invalid cached banking JSON");
  }
  const parsed = result.right;
  const normalized = makeEmptyBankingData();
  if (!isRecord(parsed)) {
    return normalized;
  }
  for (const table of BANKING_TABLES) {
    const existing = parsed[table];
    if (isRecord(existing) && isRecord(existing.data)) {
      const rows: Record<string, Record<string, unknown>> = {};
      for (const [rowId, row] of Object.entries(existing.data)) {
        if (isRecord(row)) {
          rows[rowId] = row;
        }
      }
      normalized[table] = { data: rows, notes: existing.notes ?? "" };
    }
  }
  return normalized;
}

export function loadBankingTasks(): Tau3Task[] {
  const raw = bankingTasksCache;
  if (!raw) {
    throw new Error(
      "Banking tasks not loaded — call ensureBankingTasks() first"
    );
  }
  const result = Either.try((): unknown => JSON.parse(raw));
  if (Either.isLeft(result)) {
    throw new Error("Invalid cached banking tasks JSON");
  }
  const tasksArray = result.right;
  if (!Array.isArray(tasksArray)) {
    throw new TypeError("Banking tasks is not an array");
  }
  const validated: Tau3Task[] = [];
  for (let i = 0; i < tasksArray.length; i++) {
    const task = tasksArray[i];
    const parseResult = parseSchema(Tau3TaskSchema, task);
    if (Either.isLeft(parseResult)) {
      throw new Error(
        `Invalid banking task at index ${i}: ${String(parseResult.left)}`
      );
    }
    validated.push(parseResult.right);
  }
  return validated;
}

const PROTOTYPE_KEYS = new Set(["__proto__", "constructor", "prototype"]);

function deepMergeRecord(
  base: Record<string, unknown>,
  overlay: Record<string, unknown>
): void {
  for (const [key, overlayValue] of Object.entries(overlay)) {
    if (PROTOTYPE_KEYS.has(key)) {
      throw new Error(
        `Banking initial_state overlay contains forbidden key '${key}'`
      );
    }
    const baseValue = base[key];
    if (isRecord(baseValue) && isRecord(overlayValue)) {
      deepMergeRecord(baseValue, overlayValue);
    } else {
      base[key] = overlayValue;
    }
  }
}

export function applyInitialState(db: BankingData, task: Tau3Task): void {
  const state = task.initial_state;
  if (!state) {
    return;
  }
  if (isDefinedAndNotNull(state.initialization_actions)) {
    throw new Error(
      `Banking task ${task.id} has non-null initialization_actions (not supported)`
    );
  }
  if (isDefinedAndNotNull(state.message_history)) {
    throw new Error(
      `Banking task ${task.id} has non-null message_history (not supported)`
    );
  }
  const agentData = state.initialization_data?.agent_data;
  if (!isRecord(agentData)) {
    return;
  }
  for (const [tableName, overlay] of Object.entries(agentData)) {
    if (!isBankingTableName(tableName)) {
      throw new Error(
        `Banking task ${task.id} initial_state references unknown table '${tableName}'`
      );
    }
    if (!isRecord(overlay)) {
      throw new Error(
        `Banking task ${task.id} initial_state has a non-record overlay for table '${tableName}'`
      );
    }
    const overlayData = overlay["data"];
    if (isRecord(overlayData)) {
      deepMergeRecord(db[tableName].data, overlayData);
    }
    const overlayNotes = overlay["notes"];
    if (overlayNotes !== undefined) {
      db[tableName].notes = overlayNotes;
    }
  }
}

type Hashable =
  | string
  | number
  | boolean
  | null
  | readonly Hashable[]
  | readonly [string, Hashable][];

function toHashable(item: unknown): Hashable {
  if (item === null || item === undefined) {
    return null;
  }
  if (
    typeof item === "string" ||
    typeof item === "number" ||
    typeof item === "boolean"
  ) {
    return item;
  }
  if (Array.isArray(item)) {
    return item.map(toHashable);
  }
  if (isRecord(item)) {
    const entries = Object.entries(item);
    entries.sort(([a], [b]) => a.localeCompare(b));
    return entries.map(([k, v]): [string, Hashable] => [k, toHashable(v)]);
  }
  return String(item);
}

export function dbHash(data: BankingData): string {
  const hashable = toHashable(data);
  return createHash("sha256").update(JSON.stringify(hashable)).digest("hex");
}
