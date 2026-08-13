import { Tag } from "effect/Context";
import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import {
  catchAll,
  fail,
  flatMap,
  forEach,
  gen,
  map,
  retry,
  serviceOption,
  succeed,
  sync,
  tryPromise,
} from "effect/Effect";
import { isNone } from "effect/Option";
import { intersect, recurs, spaced } from "effect/Schedule";

import { Either } from "../internal/either";
import { unknownErrorToString } from "../internal/errors";
import { wLog } from "../internal/log";
import { parseSchema, z } from "../internal/zod";
import type { GenerationIdEntry } from "./generation-ids";
import { getCollectedGenerationIdEntries } from "./generation-ids";

export interface GenerationResolverService {
  readonly resolveSourceGenerationId: (
    generationId: string
  ) => Effect<string | undefined>;
}

export class GenerationResolver extends Tag(
  "@openrouter/bench-harness/generation-resolver"
)<GenerationResolver, GenerationResolverService>() {}

const GenerationLookupSchema = z.object({
  data: z.object({
    response_cache_source_id: z.string().nullish(),
  }),
});

class GenerationLookupError extends TaggedError("GenerationLookupError")<{
  readonly message: string;
}> {}

export interface GenerationResolverConfig {
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly pollIntervalMs?: number;
  readonly maxAttempts?: number;
}

const DEFAULT_POLL_INTERVAL_MS = 5000;
const DEFAULT_MAX_ATTEMPTS = 12;
const RESOLVE_CONCURRENCY = 4;

function normalizeBaseUrl(baseUrl: string): string {
  const trimmed = baseUrl.replace(/\/+$/u, "");
  return trimmed.endsWith("/api/v1") ? trimmed : `${trimmed}/api/v1`;
}

export function makeOpenRouterGenerationResolver(
  config: GenerationResolverConfig
): GenerationResolverService {
  const baseUrl = normalizeBaseUrl(config.baseUrl ?? "https://openrouter.ai");
  const pollIntervalMs = config.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  const maxAttempts = config.maxAttempts ?? DEFAULT_MAX_ATTEMPTS;
  const lookupOnce = (
    generationId: string
  ): Effect<string, GenerationLookupError> =>
    tryPromise({
      try: async (signal) => {
        const response = await fetch(
          `${baseUrl}/generation?id=${encodeURIComponent(generationId)}`,
          {
            headers: { Authorization: `Bearer ${config.apiKey}` },
            signal,
          }
        );
        if (!response.ok) {
          await response.body?.cancel();
          throw new Error(`generation lookup returned ${response.status}`);
        }
        return (await response.json()) as unknown;
      },
      catch: (cause) =>
        new GenerationLookupError({ message: unknownErrorToString(cause) }),
    }).pipe(
      flatMap((json) => {
        const parsed = parseSchema(GenerationLookupSchema, json);
        if (Either.isLeft(parsed)) {
          return fail(
            new GenerationLookupError({
              message: unknownErrorToString(parsed.left),
            })
          );
        }
        const sourceId = parsed.right.data.response_cache_source_id;
        return sourceId === null ||
          sourceId === undefined ||
          sourceId.length === 0
          ? fail(
              new GenerationLookupError({
                message: "response_cache_source_id not present yet",
              })
            )
          : succeed(sourceId);
      })
    );
  return {
    resolveSourceGenerationId: (generationId) =>
      lookupOnce(generationId).pipe(
        retry(
          spaced(`${pollIntervalMs} millis`).pipe(
            intersect(recurs(Math.max(maxAttempts - 1, 0)))
          )
        ),
        map((sourceId): string | undefined => sourceId),
        catchAll((error) =>
          sync(() => {
            wLog("Failed to resolve cache-hit source generation id", {
              generation_id: generationId,
              error: error.message,
            });
            return undefined;
          })
        )
      ),
  };
}

function resolveEntry(
  entry: GenerationIdEntry,
  resolver: GenerationResolverService
): Effect<string> {
  return entry.isCacheHit
    ? resolver
        .resolveSourceGenerationId(entry.id)
        .pipe(map((sourceId) => sourceId ?? entry.id))
    : succeed(entry.id);
}

export const resolveCollectedGenerationIds: Effect<readonly string[]> = gen(
  function* () {
    const entries = yield* getCollectedGenerationIdEntries;
    const resolver = yield* serviceOption(GenerationResolver);
    if (isNone(resolver) || entries.every((entry) => !entry.isCacheHit)) {
      return entries.map((entry) => entry.id);
    }
    return yield* forEach(
      entries,
      (entry) => resolveEntry(entry, resolver.value),
      { concurrency: RESOLVE_CONCURRENCY }
    );
  }
);
