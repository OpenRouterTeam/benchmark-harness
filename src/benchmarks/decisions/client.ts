import { Tag } from "effect/Context";
import { millis } from "effect/Duration";
import type { Effect } from "effect/Effect";
import {
  all,
  catchTag,
  fail,
  flatMap,
  map,
  suspend,
  timeout,
  tryPromise,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed as layerSucceed } from "effect/Layer";

import { ModelError } from "../../harness/core";
import { Either } from "../../internal/either";
import { unknownErrorToString } from "../../internal/errors";
import { definedValues } from "../../internal/guards";
import { parseSchema } from "../../internal/zod";
import {
  BENCH_HARNESS_APP_REFERRER,
  BENCH_HARNESS_APP_TITLE,
} from "../../providers/app-identity";
import type { ModelErrorIdentifiers } from "../../providers/request-identifiers";
import {
  appendModelErrorIdentifiers,
  modelErrorIdentifiersFromFetchHeaders,
} from "../../providers/request-identifiers";
import { providerNameFromErrorBody } from "../../providers/responses-client";
import { filterTraceHeaders } from "../../runner/trace-headers";
import { recordGenerationId } from "../../runtime/generation-ids";
import {
  buildRequestSessionId,
  getCurrentSampleId,
} from "../../runtime/request-session-id";
import {
  buildResponseCacheSalt,
  getCurrentCallSalt,
  getCurrentEpoch,
  getCurrentRetryAttempt,
  RESPONSE_CACHE_HEADER,
  RESPONSE_CACHE_SALT_HEADER,
  RESPONSE_CACHE_SOURCE_ID_HEADER,
  RESPONSE_CACHE_STATUS_HEADER,
  RESPONSE_CACHE_STATUS_HIT,
  RESPONSE_CACHE_TTL_HEADER,
  RESPONSE_CACHE_TTL_SECONDS,
} from "../../runtime/response-cache";
import type { RetryConfig } from "../../runtime/retry";
import { rateLimitRetrySchedule, retrySalted } from "../../runtime/retry";
import type { DecisionsRequest, DecisionsResponse } from "./schema";
import { DECISIONS_ROUTE, DecisionsResponseSchema } from "./schema";

export const DEFAULT_DECISIONS_BASE_URL = "https://openrouter.ai" as const;

export const DEFAULT_DECISIONS_TIMEOUT_MS = 120_000;

export const ENDPOINT_ID_HEADER = "X-OR-Endpoint-Id" as const;

export interface DecisionsClientConfig {
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly sessionId?: string;
  readonly traceHeaders?: Readonly<Record<string, string>>;
  readonly retry?: RetryConfig;
  readonly timeoutMs?: number;
  readonly fetchImpl?: typeof fetch;
}

export interface DecisionsCall {
  readonly request: DecisionsRequest;
  readonly endpointId?: string;
}

export interface DecisionsCallResult {
  readonly response: DecisionsResponse;
  readonly latencyMs: number;
  readonly cacheHit: boolean;
  readonly identifiers: ModelErrorIdentifiers;
}

export type DecisionsClientService = {
  readonly send: (
    call: DecisionsCall
  ) => Effect<DecisionsCallResult, ModelError>;
};

export class DecisionsClient extends Tag(
  "@openrouter/bench-harness/decisions/DecisionsClient"
)<DecisionsClient, DecisionsClientService>() {}

export function decisionsUrl(baseUrl: string | undefined): string {
  const trimmed = (baseUrl ?? DEFAULT_DECISIONS_BASE_URL)
    .replace(/\/+$/u, "")
    .replace(/\/api\/v1$/u, "");
  return `${trimmed}${DECISIONS_ROUTE}`;
}

const RETRY_AFTER_HEADER = "retry-after" as const;

function retryAfterMsFrom(headers: Headers): number | undefined {
  const raw = headers.get(RETRY_AFTER_HEADER);
  if (raw === null) {
    return undefined;
  }
  const seconds = Number(raw);
  return Number.isFinite(seconds) && seconds >= 0 ? seconds * 1_000 : undefined;
}

interface SendOnceInput {
  readonly url: string;
  readonly headers: Readonly<Record<string, string>>;
  readonly call: DecisionsCall;
  readonly fetchImpl: typeof fetch;
  readonly signal: AbortSignal;
}

interface RawSendResult {
  readonly status: number;
  readonly headers: Headers;
  readonly bodyText: string;
  readonly latencyMs: number;
}

async function sendOnce(input: SendOnceInput): Promise<RawSendResult> {
  const startedAt = performance.now();
  const response = await input.fetchImpl(input.url, {
    method: "POST",
    headers: input.headers,
    body: JSON.stringify(input.call.request),
    signal: input.signal,
  });
  const bodyText = await response.text();
  return {
    status: response.status,
    headers: response.headers,
    bodyText,
    latencyMs: Math.round(performance.now() - startedAt),
  };
}

function httpError(
  raw: RawSendResult,
  identifiers: ModelErrorIdentifiers
): ModelError {
  return new ModelError(
    definedValues({
      status: raw.status,
      message: appendModelErrorIdentifiers(
        `Decisions request failed with HTTP ${raw.status}: ${raw.bodyText.slice(0, 500)}`,
        identifiers
      ),
      retryAfterMs: retryAfterMsFrom(raw.headers),
      providerName: providerNameFromErrorBody(raw.bodyText),
      ...identifiers,
    })
  );
}

function parseBody(
  raw: RawSendResult,
  identifiers: ModelErrorIdentifiers
): Either.Either<DecisionsResponse, ModelError> {
  let json: unknown;
  try {
    json = JSON.parse(raw.bodyText);
  } catch (cause) {
    return Either.left(
      new ModelError({
        message: appendModelErrorIdentifiers(
          `Decisions response was not JSON: ${unknownErrorToString(cause)}`,
          identifiers
        ),
        status: 502,
        ...identifiers,
      })
    );
  }
  const parsed = parseSchema(DecisionsResponseSchema, json);
  return Either.isLeft(parsed)
    ? Either.left(
        new ModelError({
          message: appendModelErrorIdentifiers(
            `Decisions response failed schema validation: ${parsed.left.message}`,
            identifiers
          ),
          status: 502,
          ...identifiers,
        })
      )
    : Either.right(parsed.right);
}

export function makeDecisionsClientLayer(
  config: DecisionsClientConfig
): Layer<DecisionsClient> {
  const url = decisionsUrl(config.baseUrl);
  const traceHeaders = filterTraceHeaders(config.traceHeaders);
  const fetchImpl = config.fetchImpl ?? fetch;
  const timeoutMs = config.timeoutMs ?? DEFAULT_DECISIONS_TIMEOUT_MS;

  const attempt = (
    call: DecisionsCall
  ): Effect<DecisionsCallResult, ModelError> => {
    let identifiers: ModelErrorIdentifiers = {};
    const request = suspend(() => {
      identifiers = {};
      return all({
        epoch: getCurrentEpoch,
        retryAttempt: getCurrentRetryAttempt,
        callSalt: getCurrentCallSalt,
        sampleId: getCurrentSampleId,
      }).pipe(
        flatMap(({ epoch, retryAttempt, callSalt, sampleId }) => {
          const headers: Record<string, string> = {
            Authorization: `Bearer ${config.apiKey}`,
            "Content-Type": "application/json",
            "HTTP-Referer": BENCH_HARNESS_APP_REFERRER,
            "X-OpenRouter-Title": BENCH_HARNESS_APP_TITLE,
            ...traceHeaders,
            ...definedValues({
              [ENDPOINT_ID_HEADER]: call.endpointId,
              "x-session-id": buildRequestSessionId(
                config.sessionId,
                epoch,
                sampleId
              ),
              [RESPONSE_CACHE_SALT_HEADER]: buildResponseCacheSalt(
                config.sessionId,
                epoch,
                retryAttempt,
                callSalt
              ),
            }),
            [RESPONSE_CACHE_HEADER]: "true",
            [RESPONSE_CACHE_TTL_HEADER]: `${RESPONSE_CACHE_TTL_SECONDS}`,
          };
          return tryPromise({
            try: (signal) =>
              sendOnce({ url, headers, call, fetchImpl, signal }),
            catch: (cause) =>
              new ModelError({
                message: appendModelErrorIdentifiers(
                  `Decisions request failed: ${unknownErrorToString(cause)}`,
                  identifiers
                ),
                ...identifiers,
              }),
          });
        }),
        flatMap((raw) => {
          identifiers = modelErrorIdentifiersFromFetchHeaders(raw.headers);
          if (raw.status < 200 || raw.status >= 300) {
            return fail(httpError(raw, identifiers));
          }
          const parsed = parseBody(raw, identifiers);
          if (Either.isLeft(parsed)) {
            return fail(parsed.left);
          }
          const cacheHit =
            raw.headers.get(RESPONSE_CACHE_STATUS_HEADER) ===
            RESPONSE_CACHE_STATUS_HIT;
          const cacheSourceId =
            raw.headers.get(RESPONSE_CACHE_SOURCE_ID_HEADER) ?? undefined;
          const generationId =
            cacheHit && cacheSourceId !== undefined
              ? cacheSourceId
              : parsed.right.id;
          identifiers = definedValues({ ...identifiers, generationId });
          return recordGenerationId(
            generationId,
            cacheHit,
            cacheHit && cacheSourceId !== undefined
          ).pipe(
            map((): DecisionsCallResult => ({
              response: parsed.right,
              latencyMs: raw.latencyMs,
              cacheHit,
              identifiers,
            }))
          );
        })
      );
    });
    const timed =
      timeoutMs > 0
        ? request.pipe(
            timeout(millis(timeoutMs)),
            catchTag("TimeoutException", () =>
              fail(
                new ModelError({
                  status: 408,
                  message: appendModelErrorIdentifiers(
                    `Decisions request timed out after ${timeoutMs}ms`,
                    identifiers
                  ),
                  ...identifiers,
                })
              )
            )
          )
        : request;
    return retrySalted(timed, rateLimitRetrySchedule(config.retry ?? {}));
  };

  return layerSucceed(DecisionsClient, DecisionsClient.of({ send: attempt }));
}
