import { createHash } from "node:crypto";

import type { HttpClient, HttpClientError } from "@effect/platform";
import { TaggedError } from "effect/Data";
import type { Effect, Semaphore } from "effect/Effect";
import { gen, mapError } from "effect/Effect";

import { fetchCachedTextFile } from "../../datasets/cached-file";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import type { AirlineData } from "./types";

const HF_DATASET_ID = "abhinavpola/tau2-bench-verified-airline";

const HF_REVISION = "main";

const HF_RESOLVE_BASE = `https://huggingface.co/datasets/${HF_DATASET_ID}/resolve/${HF_REVISION}`;

const AIRLINE_CACHE_MAX_AGE_MS = 24 * 60 * 60 * 1e3;

let airlineDbCache: string | undefined;

class FetchError extends TaggedError("FetchError")<{
  readonly message: string;
}> {}

function fetchHfFile(
  filename: string
): Effect<
  string,
  FetchError | HttpClientError.HttpClientError,
  HttpClient.HttpClient
> {
  return fetchCachedTextFile({
    url: `${HF_RESOLVE_BASE}/${filename}`,
    scope: `hf/${HF_DATASET_ID}`,
    revision: HF_REVISION,
    filename,
    maxAgeMs: AIRLINE_CACHE_MAX_AGE_MS,
  }).pipe(
    mapError((error) =>
      error._tag === "CachedFileError"
        ? new FetchError({
            message: `Failed to fetch ${filename} from HF (${error.status ?? error.message})`,
          })
        : error
    )
  );
}

export function ensureAirlineData(
  fetchLock: Semaphore
): Effect<
  void,
  FetchError | HttpClientError.HttpClientError,
  HttpClient.HttpClient
> {
  return fetchLock.withPermits(1)(
    gen(function* () {
      if (airlineDbCache) {
        return;
      }
      airlineDbCache = yield* fetchHfFile("db.json");
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
