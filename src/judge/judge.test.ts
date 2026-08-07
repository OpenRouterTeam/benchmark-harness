import { describe, expect, it } from "bun:test";

import {
  fail as effectFail,
  runPromise,
  runPromiseExit,
  succeed,
  suspend,
} from "effect/Effect";
import { isFailure } from "effect/Exit";

import { Either } from "../internal/either";
import { parseSchema, z } from "../internal/zod";
import type {
  ResponsesResult,
  ResponsesService,
} from "../providers/responses-client";
import { ResponsesError } from "../providers/responses-client";
import { judgeCall } from "./judge";

const VerdictSchema = z.object({ verdict: z.enum(["yes", "no"]) });

type Verdict = z.infer<typeof VerdictSchema>;

function fixtureResult(text: string): ResponsesResult {
  return {
    id: "r",
    model: "openai/gpt-4.1",
    status: "completed",
    output: [],
    usage: null,
    text,
    generationId: "g",
    provider: "OpenAI",
    generationTimeMs: 0,
  };
}

const SPEC = {
  userInput: "judge this",
  schemaName: "test_verdict",
  jsonSchema: { type: "object" },
  parseVerdict: (text: string) => {
    const parsed = parseSchema(VerdictSchema, JSON.parse(text));
    return Either.isRight(parsed)
      ? Either.right<Verdict>(parsed.right)
      : Either.left("bad verdict");
  },
};
describe("judgeCall", () => {
  it("returns the parsed verdict and sends structured-output config", async () => {
    let sentText: unknown;
    let sentModel: unknown;
    const service: ResponsesService = {
      send: (body) => {
        sentText = body.text;
        sentModel = body.model;
        return succeed(fixtureResult('{"verdict":"yes"}'));
      },
    };
    const result = await runPromise(
      judgeCall(service, { judgeModel: "openai/gpt-4.1", temperature: 0 }, SPEC)
    );
    expect(result.verdict).toEqual({ verdict: "yes" });
    expect(sentModel).toBe("openai/gpt-4.1");
    expect(sentText).toEqual({
      format: {
        type: "json_schema",
        name: "test_verdict",
        strict: true,
        schema: { type: "object" },
      },
    });
  });
  it("omits structured-output config for an unstructured judge spec", async () => {
    let sentText: unknown = "unset";
    const service: ResponsesService = {
      send: (body) => {
        sentText = body.text;
        return succeed(fixtureResult('{"verdict":"yes"}'));
      },
    };
    const result = await runPromise(
      judgeCall(
        service,
        { judgeModel: "openai/gpt-4.1" },
        {
          ...SPEC,
          jsonSchema: undefined,
        }
      )
    );
    expect(result.verdict).toEqual({ verdict: "yes" });
    expect(sentText).toBeUndefined();
  });
  it("fails with ModelError when the verdict does not parse, without retrying", async () => {
    let calls = 0;
    const service: ResponsesService = {
      send: () =>
        suspend(() => {
          calls += 1;
          return succeed(fixtureResult('{"verdict":"maybe"}'));
        }),
    };
    const exit = await runPromiseExit(
      judgeCall(service, { judgeModel: "j" }, SPEC)
    );
    expect(isFailure(exit)).toBe(true);
    expect(calls).toBe(1);
  });
  it("returns a configured parse fallback with usage and diagnostics", async () => {
    const service: ResponsesService = {
      send: () =>
        succeed({
          ...fixtureResult('{"verdict":"maybe"}'),
          usage: {
            inputTokens: 50,
            outputTokens: 10,
            totalTokens: 60,
            cost: 0.002,
          },
        }),
    };
    const result = await runPromise(
      judgeCall(
        service,
        { judgeModel: "j" },
        {
          ...SPEC,
          parseFailureFallback: { verdict: "no" } satisfies Verdict,
        }
      )
    );
    expect(result.verdict).toEqual({ verdict: "no" });
    expect(result.parseError).toContain("judge verdict parse failed");
    expect(result.usage?.totalCost).toBe(0.002);
  });
  it("retries transient errors then succeeds", async () => {
    let calls = 0;
    const service: ResponsesService = {
      send: () =>
        suspend(() => {
          calls += 1;
          if (calls === 1) {
            return effectFail(
              new ResponsesError({
                message: "429",
                status: 429,
                retryable: true,
              })
            );
          }
          return succeed(fixtureResult('{"verdict":"no"}'));
        }),
    };
    const result = await runPromise(
      judgeCall(
        service,
        { judgeModel: "j", retry: { maxRetries: 2, baseDelayMs: 1 } },
        SPEC
      )
    );
    expect(result.verdict).toEqual({ verdict: "no" });
    expect(calls).toBe(2);
  });
  it("does not retry non-retryable errors", async () => {
    let calls = 0;
    const service: ResponsesService = {
      send: () =>
        suspend(() => {
          calls += 1;
          return effectFail(
            new ResponsesError({
              message: "401",
              status: 401,
              retryable: false,
            })
          );
        }),
    };
    const exit = await runPromiseExit(
      judgeCall(
        service,
        { judgeModel: "j", retry: { maxRetries: 3, baseDelayMs: 1 } },
        SPEC
      )
    );
    expect(isFailure(exit)).toBe(true);
    expect(calls).toBe(1);
  });
});
describe("judgeCall usage", () => {
  it("returns the judge call usage/cost alongside the verdict", async () => {
    const service: ResponsesService = {
      send: () =>
        succeed({
          ...fixtureResult('{"verdict":"yes"}'),
          usage: {
            inputTokens: 50,
            outputTokens: 10,
            totalTokens: 60,
            cost: 0.002,
          },
        }),
    };
    const result = await runPromise(
      judgeCall(service, { judgeModel: "j" }, SPEC)
    );
    expect(result.usage).toEqual({
      inputTokens: 50,
      outputTokens: 10,
      totalTokens: 60,
      totalCost: 0.002,
    });
  });
});
