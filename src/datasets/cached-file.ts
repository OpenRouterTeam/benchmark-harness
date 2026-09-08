import { join } from "node:path";

import type { HttpClientError } from "@effect/platform";
import { HttpClient } from "@effect/platform";
import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { fail, gen, ignore, promise, retry, tryPromise } from "effect/Effect";

import { Either } from "../internal/either";
import { parseSchema, z } from "../internal/zod";
import type { RetryConfig } from "../runtime/retry";
import type { CacheStore } from "./cache-store";
import { resolveCacheStore } from "./cache-store";
import { hfFetchRetrySchedule } from "./huggingface";
import { encodeCacheKeySegment } from "./local-cache";

export class CachedFileError extends TaggedError("CachedFileError")<{
  readonly message: string;
  readonly status?: number;
}> {}

export interface CachedTextFileRequest {
  readonly url: string;
  readonly scope: string;
  readonly revision: string;
  readonly filename: string;
  readonly maxAgeMs?: number;
  readonly retry?: RetryConfig;
  readonly cacheStore?: CacheStore;
}

const CachedTextSchema = z.object({ text: z.string() });

function cacheKey(
  request: CachedTextFileRequest,
  store: CacheStore
): string | undefined {
  if (!store.enabled) {
    return undefined;
  }
  return join(
    "files",
    encodeCacheKeySegment(request.scope),
    encodeCacheKeySegment(request.revision),
    `${encodeCacheKeySegment(request.filename)}.json`
  );
}

function download(
  request: CachedTextFileRequest,
  client: HttpClient.HttpClient
): Effect<string, CachedFileError | HttpClientError.HttpClientError> {
  return gen(function* () {
    const response = yield* client.get(request.url);
    if (response.status < 200 || response.status >= 300) {
      return yield* fail(
        new CachedFileError({
          message: `HTTP ${response.status} for ${request.url}`,
          status: response.status,
        })
      );
    }
    return yield* response.text;
  });
}

export function fetchCachedTextFile(
  request: CachedTextFileRequest
): Effect<
  string,
  CachedFileError | HttpClientError.HttpClientError,
  HttpClient.HttpClient
> {
  return gen(function* () {
    const client = yield* HttpClient.HttpClient;
    const store = request.cacheStore ?? resolveCacheStore();
    const key = cacheKey(request, store);
    if (key !== undefined) {
      const cached = yield* promise(() =>
        store.readJson(
          key,
          request.maxAgeMs !== undefined
            ? { maxAgeMs: request.maxAgeMs }
            : undefined
        )
      );
      if (cached !== undefined) {
        const parsed = parseSchema(CachedTextSchema, cached);
        if (Either.isRight(parsed)) {
          return parsed.right.text;
        }
      }
    }
    const text = yield* download(request, client).pipe(
      retry(hfFetchRetrySchedule(request.retry))
    );
    if (key !== undefined) {
      yield* tryPromise(() => store.writeJson(key, { text })).pipe(ignore);
    }
    return text;
  });
}
