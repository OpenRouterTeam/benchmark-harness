import { afterEach, describe, expect, it } from "bun:test";

import { failureOption } from "effect/Cause";
import { flatMap, provide, runPromiseExit } from "effect/Effect";
import { getOrThrow } from "effect/Option";

import { assertFailure, assertSuccess } from "../../test/helpers/exit-asserts";
import type { CapturedRequest } from "../../test/helpers/fetch-sequence";
import { installFetchSequence } from "../../test/helpers/fetch-sequence";
import { MessageRole } from "../harness/core";
import { Model } from "../harness/model";
import { assertLeft, assertRight } from "../internal/testing";
import {
  makeSystemOneModelLayer,
  parseSystemOneAnswers,
  parseSystemOneRequest,
  systemOneRequestMessage,
} from "./systemone-model";

const REQUEST = {
  state: { fen: "8/8/8/8/8/8/8/K6k w - - 0 1", candidates: ["A", "B"] },
  questions: {
    best: {
      type: "choice" as const,
      instructions: "Which candidate is best?",
      criteria: { A: "first", B: null },
    },
  },
};

const RESPONSE = {
  id: "gen-dec-1",
  model: "typesafe/jev-1.13-20260917",
  provider: "TypeSafe",
  answers: {
    best: {
      type: "choice",
      choice: "A",
      probabilities: { A: 0.8, B: 0.2 },
      confidence: 0.7,
    },
  },
  usage: { input_tokens: 10, output_tokens: 4, cost: 0.00001 },
};

describe("systemone model", () => {
  let restore: (() => void) | undefined;
  afterEach(() => {
    restore?.();
    restore = undefined;
  });

  it("round-trips a request through the user message", () => {
    const parsed = parseSystemOneRequest([systemOneRequestMessage(REQUEST)]);
    assertRight(parsed);
    expect(parsed.right).toEqual(REQUEST);
  });

  it("rejects a user message that is not a SystemOne request", () => {
    const parsed = parseSystemOneRequest([
      { role: MessageRole.User, content: "pick the best move" },
    ]);
    assertLeft(parsed);
    expect(parsed.left.message).toContain("not JSON");
  });

  it("posts to /systemone with session and app headers and returns answers", async () => {
    const captured: CapturedRequest[] = [];
    restore = installFetchSequence([RESPONSE], captured);
    const layer = makeSystemOneModelLayer({
      model: "typesafe/jev-1.13",
      apiKey: "test-key",
      baseUrl: "https://example.test/",
      sessionId: "sess",
    });
    const exit = await runPromiseExit(
      Model.pipe(
        flatMap((model) =>
          model.generate(
            [
              { role: MessageRole.System, content: "ignored" },
              systemOneRequestMessage(REQUEST),
            ],
            { reasoningEffort: "high", temperature: 0 }
          )
        ),
        provide(layer)
      )
    );
    assertSuccess(exit);
    expect(captured).toHaveLength(1);
    expect(captured[0]?.url).toBe("https://example.test/api/v1/systemone");
    expect(captured[0]?.headers["authorization"]).toBe("Bearer test-key");
    expect(captured[0]?.headers["x-session-id"]).toBe("sess");
    expect(captured[0]?.headers["http-referer"]).toBe(
      "https://bench-harness.openrouter.ai/"
    );
    expect(captured[0]?.body).toEqual({
      model: "typesafe/jev-1.13",
      state: REQUEST.state,
      questions: REQUEST.questions,
    });
    expect(exit.value.completion).toBe(JSON.stringify(RESPONSE.answers));
    expect(exit.value.message.model).toBe("typesafe/jev-1.13-20260917");
    expect(exit.value.usage).toEqual({
      inputTokens: 10,
      outputTokens: 4,
      totalTokens: 14,
      totalCost: 0.00001,
    });
  });

  it("fails with the HTTP status when the endpoint rejects the request", async () => {
    const original = globalThis.fetch;
    globalThis.fetch = async () =>
      new Response(JSON.stringify({ error: { message: "bad" } }), {
        status: 400,
      });
    restore = () => {
      globalThis.fetch = original;
    };
    const layer = makeSystemOneModelLayer({
      model: "typesafe/jev-1.13",
      apiKey: "test-key",
    });
    const exit = await runPromiseExit(
      Model.pipe(
        flatMap((model) =>
          model.generate([systemOneRequestMessage(REQUEST)], {
            reasoningEffort: "high",
          })
        ),
        provide(layer)
      )
    );
    assertFailure(exit);
    const error = getOrThrow(failureOption(exit.cause));
    expect(error.status).toBe(400);
    expect(error.message).toContain("HTTP 400");
  });

  it("fails without retrying when the user message is not a request", async () => {
    const captured: CapturedRequest[] = [];
    restore = installFetchSequence([RESPONSE], captured);
    const layer = makeSystemOneModelLayer({
      model: "typesafe/jev-1.13",
      apiKey: "test-key",
    });
    const exit = await runPromiseExit(
      Model.pipe(
        flatMap((model) =>
          model.generate([{ role: MessageRole.User, content: "plain text" }], {
            reasoningEffort: "high",
          })
        ),
        provide(layer)
      )
    );
    assertFailure(exit);
    expect(captured).toHaveLength(0);
  });

  it("parses answers back out of a completion", () => {
    const parsed = parseSystemOneAnswers(JSON.stringify(RESPONSE.answers));
    assertRight(parsed);
    expect(parsed.right["best"]?.type).toBe("choice");
    assertLeft(parseSystemOneAnswers("not json"));
    assertLeft(parseSystemOneAnswers(JSON.stringify({ best: { type: "x" } })));
  });
});
