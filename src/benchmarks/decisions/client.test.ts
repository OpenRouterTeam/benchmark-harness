import { describe, expect, it } from "bun:test";

import {
  either,
  flatMap,
  gen,
  locally,
  map,
  provide,
  runPromise,
} from "effect/Effect";

import { ModelError } from "../../harness/core";
import { Either } from "../../internal/either";
import {
  getCollectedGenerationIdEntries,
  resetGenerationIds,
} from "../../runtime/generation-ids";
import { currentSampleIdRef } from "../../runtime/request-session-id";
import {
  currentEpochRef,
  withCallCacheSalt,
} from "../../runtime/response-cache";
import type { DecisionsCallResult, DecisionsClientConfig } from "./client";
import {
  DecisionsClient,
  decisionsUrl,
  ENDPOINT_ID_HEADER,
  makeDecisionsClientLayer,
} from "./client";
import type { DecisionsRequest } from "./schema";

const REQUEST: DecisionsRequest = {
  model: "typesafe/jev-latest",
  state: "hello",
  questions: {
    q: { type: "noul", instructions: "Is it a greeting?" },
  },
};

function abortableDelay(
  ms: number,
  signal: AbortSignal | null | undefined
): Promise<boolean> {
  const { promise, resolve, reject } = Promise.withResolvers<boolean>();
  const timer = setTimeout(() => resolve(true), ms);
  signal?.addEventListener("abort", () => {
    clearTimeout(timer);
    reject(new Error("aborted"));
  });
  return promise;
}

const OK_BODY = {
  id: "gen-123",
  model: "typesafe/jev-1.13.0",
  provider: "typesafe",
  answers: { q: { type: "noul", noul: 0.97 } },
  usage: { input_tokens: 12, output_tokens: 0, cost: 0.000001 },
};

interface Captured {
  readonly url: string;
  readonly headers: Headers;
  readonly body: unknown;
}

interface FakeResponse {
  readonly status?: number;
  readonly body: string;
  readonly headers?: Readonly<Record<string, string>>;
}

function fakeFetch(
  responses: readonly FakeResponse[],
  captured: Captured[]
): typeof fetch {
  let index = 0;
  const impl = async (
    input: string | URL | Request,
    init?: RequestInit
  ): Promise<Response> => {
    const request = input instanceof Request ? input : new Request(input, init);
    captured.push({
      url: request.url,
      headers: request.headers,
      body: await request.clone().json(),
    });
    const next = responses[Math.min(index, responses.length - 1)]!;
    index += 1;
    return new Response(next.body, {
      status: next.status ?? 200,
      headers: next.headers ?? { "content-type": "application/json" },
    });
  };
  return impl as typeof fetch;
}

function send(
  config: Omit<DecisionsClientConfig, "apiKey">,
  endpointId?: string
) {
  return resetGenerationIds.pipe(
    flatMap(() =>
      gen(function* run() {
        const client = yield* DecisionsClient;
        return yield* client.send({ request: REQUEST, endpointId });
      })
    ),
    provide(makeDecisionsClientLayer({ apiKey: "sk-test", ...config }))
  );
}

describe("decisionsUrl", () => {
  it("appends the alpha decisions route and strips a trailing /api/v1", () => {
    expect(decisionsUrl(undefined)).toBe(
      "https://openrouter.ai/api/alpha/decisions"
    );
    expect(decisionsUrl("https://example.test/api/v1/")).toBe(
      "https://example.test/api/alpha/decisions"
    );
    expect(decisionsUrl("https://example.test")).toBe(
      "https://example.test/api/alpha/decisions"
    );
  });
});

describe("makeDecisionsClientLayer", () => {
  it("posts the request with auth, app identity, routing, session, and cache headers", async () => {
    const captured: Captured[] = [];
    const result = await runPromise(
      locally(
        withCallCacheSalt(
          "q-17",
          send(
            {
              baseUrl: "https://example.test",
              sessionId: "wf-123",
              traceHeaders: { "X-Benchmark-Trace": "trace-1", cookie: "nope" },
              fetchImpl: fakeFetch(
                [{ body: JSON.stringify(OK_BODY) }],
                captured
              ),
            },
            "ep-1"
          )
        ),
        currentSampleIdRef,
        "decisions-banking77-en-base-4"
      ).pipe((effect) => locally(effect, currentEpochRef, 2))
    );
    expect(captured).toHaveLength(1);
    const [call] = captured;
    expect(call?.url).toBe("https://example.test/api/alpha/decisions");
    expect(call?.body).toEqual(REQUEST);
    expect(call?.headers.get("authorization")).toBe("Bearer sk-test");
    expect(call?.headers.get("content-type")).toBe("application/json");
    expect(call?.headers.get("http-referer")).toBe(
      "https://bench-harness.openrouter.ai/"
    );
    expect(call?.headers.get("x-openrouter-title")).toBe(
      "OpenRouter: Bench Harness"
    );
    expect(call?.headers.get(ENDPOINT_ID_HEADER)).toBe("ep-1");
    expect(call?.headers.get("x-session-id")).toBe(
      "wf-123.2.decisions-banking77-en-base-4"
    );
    expect(call?.headers.get("x-openrouter-cache-salt")).toBe(
      "wf-123:epoch-2:q-17"
    );
    expect(call?.headers.get("x-openrouter-cache")).toBe("true");
    expect(call?.headers.get("x-openrouter-cache-ttl")).toBe("7200");
    expect(call?.headers.get("x-benchmark-trace")).toBe("trace-1");
    expect(call?.headers.get("cookie")).toBeNull();

    expect(result.response.answers.q).toEqual({ type: "noul", noul: 0.97 });
    expect(result.response.usage?.input_tokens).toBe(12);
    expect(result.cacheHit).toBe(false);
    expect(result.identifiers.generationId).toBe("gen-123");
    expect(result.latencyMs).toBeGreaterThanOrEqual(0);
  });

  it("omits routing, session, and salt headers when nothing is configured", async () => {
    const captured: Captured[] = [];
    await runPromise(
      send({
        fetchImpl: fakeFetch([{ body: JSON.stringify(OK_BODY) }], captured),
      })
    );
    const [call] = captured;
    expect(call?.url).toBe("https://openrouter.ai/api/alpha/decisions");
    expect(call?.headers.get(ENDPOINT_ID_HEADER)).toBeNull();
    expect(call?.headers.get("x-session-id")).toBeNull();
    expect(call?.headers.get("x-openrouter-cache-salt")).toBeNull();
  });

  it("records the generation id and marks cache hits from response headers", async () => {
    const entries = await runPromise(
      send({
        fetchImpl: fakeFetch(
          [
            {
              body: JSON.stringify(OK_BODY),
              headers: {
                "content-type": "application/json",
                "x-openrouter-cache-status": "HIT",
                "x-openrouter-cache-source-id": "gen-source",
              },
            },
          ],
          []
        ),
      }).pipe(
        flatMap((result: DecisionsCallResult) =>
          getCollectedGenerationIdEntries.pipe(
            map((collected) => ({ result, collected }))
          )
        )
      )
    );
    expect(entries.result.cacheHit).toBe(true);
    expect(entries.result.identifiers.generationId).toBe("gen-source");
    expect(entries.collected).toEqual([
      {
        id: "gen-source",
        isCacheHit: true,
        countsTowardUsage: true,
        isResolvedSource: true,
      },
    ]);
  });

  it("fails with a ModelError carrying status, provider, and request identifiers on non-2xx", async () => {
    const outcome = await runPromise(
      either(
        send({
          retry: { maxRetries: 0 },
          fetchImpl: fakeFetch(
            [
              {
                status: 429,
                body: JSON.stringify({
                  error: {
                    message: "slow down",
                    metadata: { provider_name: "typesafe" },
                  },
                }),
                headers: {
                  "content-type": "application/json",
                  "retry-after": "3",
                  "cf-ray": "ray-1",
                  "x-request-id": "req-1",
                },
              },
            ],
            []
          ),
        })
      )
    );
    expect(Either.isLeft(outcome)).toBe(true);
    const error = Either.isLeft(outcome) ? outcome.left : undefined;
    expect(error).toBeInstanceOf(ModelError);
    expect(error?.status).toBe(429);
    expect(error?.retryAfterMs).toBe(3_000);
    expect(error?.providerName).toBe("typesafe");
    expect(error?.cfRay).toBe("ray-1");
    expect(error?.xRequestId).toBe("req-1");
    expect(error?.message).toContain("HTTP 429");
  });

  it("retries a retryable status and succeeds on the next attempt", async () => {
    const captured: Captured[] = [];
    const result = await runPromise(
      send({
        retry: { maxRetries: 1, baseDelayMs: 1 },
        fetchImpl: fakeFetch(
          [
            { status: 503, body: "upstream unavailable" },
            { body: JSON.stringify(OK_BODY) },
          ],
          captured
        ),
      })
    );
    expect(captured).toHaveLength(2);
    expect(result.response.model).toBe("typesafe/jev-1.13.0");
  });

  it("rejects a non-JSON body and a body that fails the response schema", async () => {
    const nonJson = await runPromise(
      either(
        send({
          retry: { maxRetries: 0 },
          fetchImpl: fakeFetch([{ body: "<html>oops</html>" }], []),
        })
      )
    );
    const nonJsonError = Either.isLeft(nonJson) ? nonJson.left : undefined;
    expect(nonJsonError?.status).toBe(502);
    expect(nonJsonError?.message).toContain("not JSON");

    const badShape = await runPromise(
      either(
        send({
          retry: { maxRetries: 0 },
          fetchImpl: fakeFetch(
            [
              {
                body: JSON.stringify({
                  model: "m",
                  answers: { q: { type: "noul", noul: "high" } },
                }),
              },
            ],
            []
          ),
        })
      )
    );
    const badShapeError = Either.isLeft(badShape) ? badShape.left : undefined;
    expect(badShapeError?.status).toBe(502);
    expect(badShapeError?.message).toContain("schema validation");
  });

  it("times out with a 408 ModelError when the request exceeds the deadline", async () => {
    const slowFetch = (async (_input, init) => {
      await abortableDelay(5_000, init?.signal);
      return new Response(JSON.stringify(OK_BODY));
    }) as typeof fetch;
    const outcome = await runPromise(
      either(
        send({
          retry: { maxRetries: 0 },
          timeoutMs: 20,
          fetchImpl: slowFetch,
        })
      )
    );
    const error = Either.isLeft(outcome) ? outcome.left : undefined;
    expect(error?.status).toBe(408);
    expect(error?.message).toContain("timed out after 20ms");
  });
});
