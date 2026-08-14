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
import { isDefinedAndNotNull } from "../internal/guards";
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
  readonly sourceIds: readonly string[];
  readonly usage?: ReplayedUsage;
}

export interface GenerationResolverService {
  readonly resolveSourceGeneration: (
    generationId: string,
    options?: {
      readonly includeUsage?: boolean;
      readonly includeRelated?: boolean;
    }
  ) => Effect<ResolvedSourceGeneration | undefined>;
}

export class GenerationResolver extends Tag(
  "@openrouter/bench-harness/generation-resolver"
)<GenerationResolver, GenerationResolverService>() {}

const GenerationLookupSchema = z.object({
  data: z.object({
    response_cache_source_id: z.string().nullish(),
    native_tokens_prompt: z.number().nullish(),
    native_tokens_completion: z.number().nullish(),
    native_tokens_reasoning: z.number().nullish(),
    total_cost: z.number().nullish(),
    generation_time: z.number().nullish(),
    related_generation_ids: z.array(z.string()).nullish(),
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
const RESOLVE_CONCURRENCY = 8;
const MAX_RELATED_DEPTH = 16;

function normalizeBaseUrl(baseUrl: string): string {
  const trimmed = baseUrl.replace(/\/+$/u, "");
  return trimmed.endsWith("/api/v1") ? trimmed : `${trimmed}/api/v1`;
}

function usageFromLookup(
  data: GenerationLookupData,
  generationId: string
): ReplayedUsage {
  const hasUsageFields =
    isDefinedAndNotNull(data.native_tokens_prompt) ||
    isDefinedAndNotNull(data.native_tokens_completion) ||
    isDefinedAndNotNull(data.total_cost);
  if (!hasUsageFields) {
    wLog("Source generation has no usage fields, folding zeros", {
      generation_id: generationId,
    });
  }
  const inputTokens = data.native_tokens_prompt ?? 0;
  const outputTokens = data.native_tokens_completion ?? 0;
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
  const pollSchedule = (attempts: number) =>
    spaced(`${pollIntervalMs} millis`).pipe(
      intersect(recurs(Math.max(attempts - 1, 0))),
      whileInput((error: GenerationLookupError) => error.retryable)
    );
  const lookupOnce = (
    generationId: string,
    includeRelated: boolean
  ): Effect<GenerationLookupData, GenerationLookupError> =>
    tryPromise({
      try: async (
        signal
      ): Promise<{ readonly status: number } | { readonly json: unknown }> => {
        const response = await fetch(
          `${baseUrl}/generation?id=${encodeURIComponent(generationId)}${includeRelated ? "&include_related=true" : ""}`,
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
  const lookupGeneration = (
    generationId: string,
    includeRelated = false
  ): Effect<GenerationLookupData, GenerationLookupError> =>
    lookupOnce(generationId, includeRelated).pipe(
      retry(pollSchedule(maxAttempts))
    );
  const resolveSourceGeneration = (
    generationId: string,
    options:
      | {
          readonly includeUsage?: boolean;
          readonly includeRelated?: boolean;
        }
      | undefined,
    visited: ReadonlySet<string>,
    depth: number
  ): Effect<ResolvedSourceGeneration | undefined> => {
    if (depth >= MAX_RELATED_DEPTH || visited.has(generationId)) {
      return sync(() => {
        wLog("Generation relation resolution reached a cycle or depth limit", {
          generation_id: generationId,
        });
        return undefined;
      });
    }
    const nextVisited = new Set(visited).add(generationId);
    const includeRelated = options?.includeRelated === true;
    return lookupGeneration(generationId, includeRelated).pipe(
      flatMap((data) => {
        const cacheSourceId = data.response_cache_source_id;
        if (cacheSourceId !== null && cacheSourceId !== undefined) {
          if (options?.includeUsage === false && !includeRelated) {
            return succeed<ResolvedSourceGeneration>({
              sourceIds: [cacheSourceId],
            });
          }
          return resolveSourceGeneration(
            cacheSourceId,
            options,
            nextVisited,
            depth + 1
          ).pipe(
            map(
              (resolved): ResolvedSourceGeneration =>
                resolved ?? { sourceIds: [cacheSourceId] }
            )
          );
        }
        const relatedIds = includeRelated
          ? (data.related_generation_ids ?? [])
          : [];
        if (relatedIds.length === 0) {
          return succeed<ResolvedSourceGeneration>({
            sourceIds: [generationId],
            ...(options?.includeUsage === false
              ? {}
              : { usage: usageFromLookup(data, generationId) }),
          });
        }
        return forEach(
          relatedIds,
          (relatedId) =>
            resolveSourceGeneration(relatedId, options, nextVisited, depth + 1),
          { concurrency: RESOLVE_CONCURRENCY }
        ).pipe(
          map((resolved) => {
            if (resolved.some((entry) => entry === undefined)) {
              return undefined;
            }
            const entries = resolved.filter(isDefinedAndNotNull);
            let usage: ReplayedUsage | undefined;
            for (const entry of entries) {
              usage = sumReplayedUsage(usage, entry.usage);
            }
            return {
              sourceIds: entries.flatMap((entry) => entry.sourceIds),
              ...(usage !== undefined && { usage }),
            } satisfies ResolvedSourceGeneration;
          })
        );
      }),
      catchAll((error) =>
        sync(() => {
          wLog("Failed to resolve generation relations", {
            generation_id: generationId,
            error: error.message,
          });
          return undefined;
        })
      )
    );
  };
  return {
    resolveSourceGeneration: (generationId, options) =>
      resolveSourceGeneration(generationId, options, new Set(), 0),
  };
}

export interface ResolvedGenerations {
  readonly ids: readonly string[];
  readonly replayedUsage?: ReplayedUsage;
}

interface ResolvedEntry {
  readonly ids: readonly string[];
  readonly usage?: ReplayedUsage;
}

function resolveEntry(
  entry: GenerationIdEntry,
  resolver: GenerationResolverService
): Effect<ResolvedEntry> {
  if (entry.isResolvedSource && !entry.countsTowardUsage) {
    return succeed({ ids: [entry.id] });
  }
  return entry.isCacheHit || entry.shouldResolveChildren
    ? resolver
        .resolveSourceGeneration(entry.id, {
          includeUsage: entry.isCacheHit && entry.countsTowardUsage,
          includeRelated: entry.shouldResolveChildren,
        })
        .pipe(
          map((resolved): ResolvedEntry =>
            resolved === undefined
              ? { ids: [entry.id] }
              : {
                  ids: resolved.sourceIds,
                  ...(resolved.usage && { usage: resolved.usage }),
                }
          )
        )
    : succeed({ ids: [entry.id] });
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
    if (
      isNone(resolver) ||
      entries.every(
        (entry) => !entry.isCacheHit && !entry.shouldResolveChildren
      )
    ) {
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
      ids: resolved.flatMap((entry) => entry.ids),
      ...(replayedUsage !== undefined && { replayedUsage }),
    };
  }
);
