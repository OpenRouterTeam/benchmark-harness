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
  readonly retry?: RetryConfig;
  readonly cacheStore?: CacheStore;
}

const CachedTextSchema = z.object({ text: z.string() });

function download(
  url: string,
  client: HttpClient.HttpClient
): Effect<string, CachedFileError | HttpClientError.HttpClientError> {
  return gen(function* () {
    const response = yield* client.get(url);
    if (response.status < 200 || response.status >= 300) {
      return yield* fail(
        new CachedFileError({
          message: `HTTP ${response.status} for ${url}`,
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
    const key = `files/${encodeCacheKeySegment(request.url)}.json`;
    const cached = parseSchema(
      CachedTextSchema,
      yield* promise(() => store.readJson(key))
    );
    if (Either.isRight(cached)) {
      return cached.right.text;
    }
    const text = yield* download(request.url, client).pipe(
      retry(hfFetchRetrySchedule(request.retry))
    );
    yield* tryPromise(() => store.writeJson(key, { text })).pipe(ignore);
    return text;
  });
}
