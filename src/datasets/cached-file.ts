import type { HttpClientError } from "@effect/platform";
import { HttpClient } from "@effect/platform";
import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { fail, gen, ignore, promise, retry, tryPromise } from "effect/Effect";

import { Either } from "../internal/either";
import { definedValues, isRecord } from "../internal/guards";
import { parseSchema, z } from "../internal/zod";
import type { RetryConfig } from "../runtime/retry";
import type { CacheStore } from "./cache-store";
import { resolveCacheStore } from "./cache-store";
import { hfFetchRetrySchedule, resolveHfToken } from "./huggingface";
import { encodeCacheKeySegment } from "./local-cache";

export class CachedFileError extends TaggedError("CachedFileError")<{
  readonly message: string;
  readonly status?: number;
  readonly retryAfterMs?: number;
}> {}

export type CachedTextValidator = (text: string) => string | undefined;

export interface CachedTextFileRequest {
  readonly url: string;
  readonly retry?: RetryConfig;
  readonly cacheStore?: CacheStore;
  readonly hfToken?: string;
  readonly validate?: CachedTextValidator;
}

type CachedFileFailure = CachedFileError | HttpClientError.HttpClientError;

const CachedTextSchema = z.object({ text: z.string() });

const HF_HOST = "huggingface.co";

export function isHuggingFaceUrl(url: string): boolean {
  const parsed = Either.try(() => new URL(url));
  if (Either.isLeft(parsed)) {
    return false;
  }
  const { hostname } = parsed.right;
  return hostname === HF_HOST || hostname.endsWith(`.${HF_HOST}`);
}

export function parseRetryAfterMs(
  value: string | undefined,
  now: number = Date.now()
): number | undefined {
  const normalized = value?.trim();
  if (normalized === undefined || normalized === "") {
    return undefined;
  }
  const seconds = Number(normalized);
  if (Number.isFinite(seconds)) {
    return seconds >= 0 ? seconds * 1e3 : undefined;
  }
  const at = Date.parse(normalized);
  return Number.isFinite(at) ? Math.max(0, at - now) : undefined;
}

export function isRetryableCachedFileFailure(
  error: CachedFileFailure
): boolean {
  if (error._tag !== "CachedFileError") {
    return true;
  }
  return error.status === 429 || (error.status ?? 0) >= 500;
}

export function jsonTextValidator(
  expected: "object" | "array"
): CachedTextValidator {
  return (text) => {
    const parsed = Either.try((): unknown => JSON.parse(text));
    if (Either.isLeft(parsed)) {
      return "body is not valid JSON";
    }
    const value = parsed.right;
    if (expected === "array") {
      return Array.isArray(value) ? undefined : "body is not a JSON array";
    }
    return isRecord(value) ? undefined : "body is not a JSON object";
  };
}

function cachedFileRetryAfterMs(error: CachedFileFailure): number | undefined {
  return error._tag === "CachedFileError" ? error.retryAfterMs : undefined;
}

function authorizationHeaders(
  url: string,
  hfToken: string
): Readonly<Record<string, string>> | undefined {
  if (hfToken === "" || !isHuggingFaceUrl(url)) {
    return undefined;
  }
  return { Authorization: `Bearer ${hfToken}` };
}

function download(
  url: string,
  hfToken: string,
  client: HttpClient.HttpClient
): Effect<string, CachedFileFailure> {
  return gen(function* () {
    const headers = authorizationHeaders(url, hfToken);
    const response = yield* client.get(
      url,
      headers !== undefined ? { headers } : undefined
    );
    if (response.status < 200 || response.status >= 300) {
      yield* ignore(response.text);
      return yield* fail(
        new CachedFileError(
          definedValues({
            message: `HTTP ${response.status} for ${url}`,
            status: response.status,
            retryAfterMs: parseRetryAfterMs(response.headers["retry-after"]),
          })
        )
      );
    }
    return yield* response.text;
  });
}

export function fetchCachedTextFile(
  request: CachedTextFileRequest
): Effect<string, CachedFileFailure, HttpClient.HttpClient> {
  return gen(function* () {
    const client = yield* HttpClient.HttpClient;
    const store = request.cacheStore ?? resolveCacheStore();
    const key = `files/${encodeCacheKeySegment(request.url)}.json`;
    if (store.enabled) {
      const cached = parseSchema(
        CachedTextSchema,
        yield* promise(() => store.readJson(key))
      );
      if (Either.isRight(cached)) {
        return cached.right.text;
      }
    }
    const hfToken = request.hfToken ?? (yield* resolveHfToken());
    const text = yield* download(request.url, hfToken, client).pipe(
      retry(
        hfFetchRetrySchedule<CachedFileFailure>(
          request.retry,
          isRetryableCachedFileFailure,
          cachedFileRetryAfterMs
        )
      )
    );
    const invalid = request.validate?.(text);
    if (invalid !== undefined) {
      return yield* fail(
        new CachedFileError({
          message: `Invalid response for ${request.url}: ${invalid}`,
        })
      );
    }
    if (store.enabled) {
      yield* tryPromise(() => store.writeJson(key, { text })).pipe(ignore);
    }
    return text;
  });
}
