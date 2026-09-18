import { describe, expect, it } from "bun:test";

import { either, flatMap, map, runPromise } from "effect/Effect";

import { ModelError } from "../harness/core";
import { Either } from "../internal/either";
import {
  getCollectedGenerationIds,
  resetGenerationIds,
} from "../runtime/generation-ids";
import {
  decisionsUrl,
  isDecisionsModel,
  makeDecisionsService,
} from "./decisions-client";

const REQUEST = {
  model: "~typesafe/jev-latest",
  state: { dossier: "text" },
  instructions: "pick one",
  criteria: { A: "first", B: "second" },
} as const;

function withFetch<T>(
  handler: (request: Request) => Promise<Response>,
  run: () => Promise<T>
): Promise<T> {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (input, init) =>
    handler(input instanceof Request ? input : new Request(input, init));
  return run().finally(() => {
    globalThis.fetch = originalFetch;
  });
}

describe("isDecisionsModel", () => {
  it("recognises typesafe models with or without the alias prefix and variants", () => {
    expect(isDecisionsModel("~typesafe/jev-latest")).toBe(true);
    expect(isDecisionsModel("typesafe/jev-1:nitro")).toBe(true);
    expect(isDecisionsModel("openai/gpt-5-mini")).toBe(false);
    expect(isDecisionsModel("~openai/gpt-luna-latest")).toBe(false);
  });
});

describe("decisionsUrl", () => {
  it("derives the alpha endpoint from the base URL origin", () => {
    expect(decisionsUrl()).toBe("https://openrouter.ai/api/alpha/decisions");
    expect(decisionsUrl("https://example.test/api/v1")).toBe(
      "https://example.test/api/alpha/decisions"
    );
  });
});

describe("makeDecisionsService", () => {
  it("posts a choice question, validates the answer and records the generation id", async () => {
    let captured: Request | undefined;
    const { result, ids } = await withFetch(
      async (request) => {
        captured = request;
        return Response.json({
          id: "gen-decision-1",
          model: "typesafe/jev-1",
          answers: {
            decision: {
              type: "choice",
              choice: "B",
              probabilities: { A: 0.2, B: 0.8 },
            },
          },
          usage: { input_tokens: 40, output_tokens: 3, cost: 0.0005 },
        });
      },
      () =>
        runPromise(
          resetGenerationIds.pipe(
            flatMap(() =>
              makeDecisionsService({
                apiKey: "sk-test",
                baseUrl: "https://example.test/api/v1",
                sessionId: "session-1",
                traceHeaders: { traceparent: "00-abc", cookie: "nope" },
              }).choose(REQUEST, { temperature: 0, providerOnly: ["p1"] })
            ),
            flatMap((result) =>
              getCollectedGenerationIds.pipe(map((ids) => ({ result, ids })))
            )
          )
        )
    );
    expect(ids).toEqual(["gen-decision-1"]);
    expect(result.probabilities).toEqual({ A: 0.2, B: 0.8 });
    expect(result.usage).toEqual({
      inputTokens: 40,
      outputTokens: 3,
      totalTokens: 43,
      reasoningTokens: 0,
      totalCost: 0.0005,
    });
    expect(captured?.url).toBe("https://example.test/api/alpha/decisions");
    expect(captured?.method).toBe("POST");
    expect(captured?.headers.get("authorization")).toBe("Bearer sk-test");
    expect(captured?.headers.get("x-session-id")).toBe("session-1");
    expect(captured?.headers.get("traceparent")).toBe("00-abc");
    expect(captured?.headers.get("cookie")).toBeNull();
    expect(captured?.headers.get("x-openrouter-title")).toBe(
      "OpenRouter: Bench Harness"
    );
    expect(await captured?.json()).toEqual({
      model: "~typesafe/jev-latest",
      state: { dossier: "text" },
      questions: {
        decision: {
          type: "choice",
          instructions: "pick one",
          criteria: { A: "first", B: "second" },
        },
      },
      provider: { only: ["p1"] },
      session_id: "session-1",
    });
  });

  it("falls back to a one-hot distribution when probabilities are omitted", async () => {
    const result = await withFetch(
      async () =>
        Response.json({
          answers: { decision: { type: "choice", choice: "A" } },
          usage: { input_tokens: 1, output_tokens: 1 },
        }),
      () =>
        runPromise(
          makeDecisionsService({ apiKey: "sk-test" }).choose(REQUEST, {
            temperature: 0,
          })
        )
    );
    expect(result.probabilities).toEqual({ A: 1, B: 0 });
    expect(result.usage.totalCost).toBe(0);
  });

  it("maps HTTP failures to ModelError with status and identifiers", async () => {
    const outcome = await withFetch(
      async () =>
        new Response(JSON.stringify({ error: { message: "nope" } }), {
          status: 402,
          headers: { "x-openrouter-request-id": "req-1" },
        }),
      () =>
        runPromise(
          either(
            makeDecisionsService({ apiKey: "sk-test" }).choose(REQUEST, {
              temperature: 0,
            })
          )
        )
    );
    expect(Either.isLeft(outcome)).toBe(true);
    const error = Either.isLeft(outcome) ? outcome.left : undefined;
    expect(error).toBeInstanceOf(ModelError);
    expect(error?.status).toBe(402);
    expect(error?.message).toContain("Decisions HTTP 402");
    expect(error?.message).toContain("nope");
  });

  it("rejects responses that fail schema validation or omit the decision answer", async () => {
    const malformed = await withFetch(
      async () => Response.json({ answers: {}, usage: {} }),
      () =>
        runPromise(
          either(
            makeDecisionsService({ apiKey: "sk-test" }).choose(REQUEST, {
              temperature: 0,
            })
          )
        )
    );
    expect(Either.isLeft(malformed)).toBe(true);
    expect(Either.isLeft(malformed) ? malformed.left.message : "").toContain(
      "failed validation"
    );

    const missing = await withFetch(
      async () =>
        Response.json({
          answers: { other: { type: "choice", choice: "A" } },
          usage: { input_tokens: 1, output_tokens: 1 },
        }),
      () =>
        runPromise(
          either(
            makeDecisionsService({ apiKey: "sk-test" }).choose(REQUEST, {
              temperature: 0,
            })
          )
        )
    );
    expect(Either.isLeft(missing)).toBe(true);
    expect(Either.isLeft(missing) ? missing.left.message : "").toContain(
      'omitted the "decision" answer'
    );
  });
});
