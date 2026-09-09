import { createHash } from "node:crypto";

import type { HttpClient, HttpClientError } from "@effect/platform";
import type { Effect, Semaphore } from "effect/Effect";
import { gen } from "effect/Effect";

import type { CachedFileError } from "../../datasets/cached-file";
import {
  fetchCachedTextFile,
  jsonTextValidator,
} from "../../datasets/cached-file";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import type { AirlineData } from "./types";

export const TAU_BENCH_AIRLINE_DATASET_ID =
  "abhinavpola/tau2-bench-verified-airline";

export const TAU_BENCH_AIRLINE_REVISION =
  "790bdd0336f4e3386824ced48ee2a98a11058345";

const AIRLINE_DB_URL = `https://huggingface.co/datasets/${TAU_BENCH_AIRLINE_DATASET_ID}/resolve/${TAU_BENCH_AIRLINE_REVISION}/db.json`;

let airlineDbCache: string | undefined;

export function ensureAirlineData(
  fetchLock: Semaphore
): Effect<
  void,
  CachedFileError | HttpClientError.HttpClientError,
  HttpClient.HttpClient
> {
  return fetchLock.withPermits(1)(
    gen(function* () {
      if (airlineDbCache) {
        return;
      }
      airlineDbCache = yield* fetchCachedTextFile({
        url: AIRLINE_DB_URL,
        validate: jsonTextValidator("object"),
      });
    })
  );
}

export function seedAirlineDataCache(data: AirlineData): void {
  airlineDbCache = JSON.stringify(data);
}

export function loadAirlineData(): AirlineData {
  const raw = airlineDbCache;
  if (!raw) {
    throw new Error("Airline data not loaded — call ensureAirlineData() first");
  }
  const result = Either.try((): AirlineData => JSON.parse(raw));
  if (Either.isLeft(result)) {
    throw new Error("Invalid cached airline JSON");
  }
  return result.right;
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

export function dbHash(data: AirlineData): string {
  const hashable = toHashable(data);
  return createHash("sha256").update(JSON.stringify(hashable)).digest("hex");
}
