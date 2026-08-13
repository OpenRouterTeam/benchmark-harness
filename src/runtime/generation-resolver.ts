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
import { intersect, recurs, spaced, whileInput } from "effect/Schedule";

import { Either } from "../internal/either";
import { unknownErrorToString } from "../internal/errors";
import { wLog } from "../internal/log";
import { parseSchema, z } from "../internal/zod";
import type { GenerationIdEntry } from "./generation-ids";
import { getCollectedGenerationIdEntries } from "./generation-ids";

export interface ReplayedUsage {
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly totalTokens: number;
  readonly reasoningTokens: number;
  readonly totalCost: number;
  readonly generationTimeMs: number;
}

export interface ResolvedSourceGeneration {
  readonly sourceId: string;
  readonly usage?: ReplayedUsage;
}

export interface GenerationResolverService {
  readonly resolveSourceGeneration: (
    generationId: string,
    options?: { readonly includeUsage?: boolean }
  ) => Effect<ResolvedSourceGeneration | undefined>;
}

export class GenerationResolver extends Tag(
  "@openrouter/bench-harness/generation-resolver"
)<GenerationResolver, GenerationResolverService>() {}

const GenerationLookupSchema = z.object({
  data: z.object({
    response_cache_source_id: z.string().nullish(),
    tokens_prompt: z.number().nullish(),
    tokens_completion: z.number().nullish(),
    native_tokens_reasoning: z.number().nullish(),
    total_cost: z.number().nullish(),
    generation_time: z.number().nullish(),
  }),
});

type GenerationLookupData = z.infer<typeof GenerationLookupSchema>["data"];

class GenerationLookupError extends TaggedError("GenerationLookupError")<{
  readonly message: string;
  readonly retryable: boolean;
}> {}

function isRetryableLookupStatus(status: number): boolean {
  return status === 404 || status === 408 || status === 429 || status >= 500;
}

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

function usageFromLookup(data: GenerationLookupData): ReplayedUsage {
  const inputTokens = data.tokens_prompt ?? 0;
  const outputTokens = data.tokens_completion ?? 0;
  return {
    inputTokens,
    outputTokens,
    totalTokens: inputTokens + outputTokens,
    reasoningTokens: data.native_tokens_reasoning ?? 0,
    totalCost: data.total_cost ?? 0,
    generationTimeMs: data.generation_time ?? 0,
  };
}

export function makeOpenRouterGenerationResolver(
  config: GenerationResolverConfig
): GenerationResolverService {
  const baseUrl = normalizeBaseUrl(config.baseUrl ?? "https://openrouter.ai");
  const pollIntervalMs = config.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS;
  const maxAttempts = config.maxAttempts ?? DEFAULT_MAX_ATTEMPTS;
  const pollSchedule = () =>
    spaced(`${pollIntervalMs} millis`).pipe(
      intersect(recurs(Math.max(maxAttempts - 1, 0))),
      whileInput((error: GenerationLookupError) => error.retryable)
    );
  const lookupOnce = (
    generationId: string
  ): Effect<GenerationLookupData, GenerationLookupError> =>
    tryPromise({
      try: async (
        signal
      ): Promise<{ readonly status: number } | { readonly json: unknown }> => {
        const response = await fetch(
          `${baseUrl}/generation?id=${encodeURIComponent(generationId)}`,
          {
            headers: { Authorization: `Bearer ${config.apiKey}` },
            signal,
          }
        );
        if (!response.ok) {
          await response.body?.cancel();
          return { status: response.status };
        }
        return { json: (await response.json()) as unknown };
      },
      catch: (cause) =>
        new GenerationLookupError({
          message: unknownErrorToString(cause),
          retryable: true,
        }),
    }).pipe(
      flatMap((result) => {
        if ("status" in result) {
          return fail(
            new GenerationLookupError({
              message: `generation lookup returned ${result.status}`,
              retryable: isRetryableLookupStatus(result.status),
            })
          );
        }
        const parsed = parseSchema(GenerationLookupSchema, result.json);
        return Either.isLeft(parsed)
          ? fail(
              new GenerationLookupError({
                message: unknownErrorToString(parsed.left),
                retryable: false,
              })
            )
          : succeed(parsed.right.data);
      })
    );
  const lookupSourceId = (
    generationId: string
  ): Effect<string, GenerationLookupError> =>
    lookupOnce(generationId).pipe(
      flatMap((data) => {
        const sourceId = data.response_cache_source_id;
        return sourceId === null ||
          sourceId === undefined ||
          sourceId.length === 0
          ? fail(
              new GenerationLookupError({
                message: "response_cache_source_id not present yet",
                retryable: true,
              })
            )
          : succeed(sourceId);
      }),
      retry(pollSchedule())
    );
  const lookupSourceUsage = (
    sourceId: string
  ): Effect<ReplayedUsage | undefined> =>
    lookupOnce(sourceId).pipe(
      retry(pollSchedule()),
      map((data): ReplayedUsage | undefined => usageFromLookup(data)),
      catchAll((error) =>
        sync(() => {
          wLog("Failed to fetch source generation usage", {
            generation_id: sourceId,
            error: error.message,
          });
          return undefined;
        })
      )
    );
  return {
    resolveSourceGeneration: (generationId, options) =>
      lookupSourceId(generationId).pipe(
        flatMap((sourceId) =>
          options?.includeUsage === false
            ? succeed<ResolvedSourceGeneration>({ sourceId })
            : lookupSourceUsage(sourceId).pipe(
                map((usage): ResolvedSourceGeneration => ({
                  sourceId,
                  ...(usage !== undefined && { usage }),
                }))
              )
        ),
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

export interface ResolvedGenerations {
  readonly ids: readonly string[];
  readonly replayedUsage?: ReplayedUsage;
}

interface ResolvedEntry {
  readonly id: string;
  readonly usage?: ReplayedUsage;
}

function resolveEntry(
  entry: GenerationIdEntry,
  resolver: GenerationResolverService
): Effect<ResolvedEntry> {
  return entry.isCacheHit
    ? resolver
        .resolveSourceGeneration(entry.id, {
          includeUsage: entry.countsTowardUsage,
        })
        .pipe(
          map((resolved): ResolvedEntry =>
            resolved === undefined
              ? { id: entry.id }
              : {
                  id: resolved.sourceId,
                  ...(resolved.usage && { usage: resolved.usage }),
                }
          )
        )
    : succeed({ id: entry.id });
}

function sumReplayedUsage(
  acc: ReplayedUsage | undefined,
  usage: ReplayedUsage | undefined
): ReplayedUsage | undefined {
  if (usage === undefined) {
    return acc;
  }
  if (acc === undefined) {
    return usage;
  }
  return {
    inputTokens: acc.inputTokens + usage.inputTokens,
    outputTokens: acc.outputTokens + usage.outputTokens,
    totalTokens: acc.totalTokens + usage.totalTokens,
    reasoningTokens: acc.reasoningTokens + usage.reasoningTokens,
    totalCost: acc.totalCost + usage.totalCost,
    generationTimeMs: acc.generationTimeMs + usage.generationTimeMs,
  };
}

export const resolveCollectedGenerations: Effect<ResolvedGenerations> = gen(
  function* () {
    const entries = yield* getCollectedGenerationIdEntries;
    const resolver = yield* serviceOption(GenerationResolver);
    if (isNone(resolver) || entries.every((entry) => !entry.isCacheHit)) {
      return { ids: entries.map((entry) => entry.id) };
    }
    const resolved = yield* forEach(
      entries,
      (entry) => resolveEntry(entry, resolver.value),
      { concurrency: RESOLVE_CONCURRENCY }
    );
    let replayedUsage: ReplayedUsage | undefined;
    for (const entry of resolved) {
      replayedUsage = sumReplayedUsage(replayedUsage, entry.usage);
    }
    return {
      ids: resolved.map((entry) => entry.id),
      ...(replayedUsage !== undefined && { replayedUsage }),
    };
  }
);
