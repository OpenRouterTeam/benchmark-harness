import { afterEach, describe, expect, it } from "bun:test";
import { readFile } from "node:fs/promises";

import { FetchHttpClient } from "@effect/platform";
import { gen, provide, runPromiseExit } from "effect/Effect";
import { provide as layerProvide } from "effect/Layer";

import { assertSuccess } from "../../test/helpers/exit-asserts";
import { Model } from "../harness/model";
import { makeOpenRouterModelLayer } from "./openrouter-model";

function withFunctionCall(stream: string): string {
  return stream
    .split("\n")
    .map((line) => {
      if (!line.startsWith("data: ") || line === "data: [DONE]") {
        return line;
      }
      const event: unknown = JSON.parse(line.slice(6));
      if (
        typeof event !== "object" ||
        event === null ||
        !("type" in event) ||
        event.type !== "response.completed"
      ) {
        return line;
      }
      const response = event.response;
      if (
        typeof response !== "object" ||
        response === null ||
        !("output" in response) ||
        !Array.isArray(response.output)
      ) {
        return line;
      }
      return `data: ${JSON.stringify({
        ...event,
        response: {
          ...response,
          output: [
            ...response.output,
            {
              type: "function_call",
              id: "item-1",
              call_id: "call-1",
              name: "lookup",
              arguments: '{"x":1}',
            },
          ],
        },
      })}`;
    })
    .join("\n");
}

describe("openrouter-model", () => {
  let restore: (() => void) | undefined;

  afterEach(() => {
    restore?.();
    restore = undefined;
  });

  it("uses the Responses endpoint with streaming and cache control", async () => {
    const stream = withFunctionCall(
      await readFile(
        new URL(
          "../../test/fixtures/advisor-responses-stream.sse",
          import.meta.url
        ),
        "utf8"
      )
    );
    const originalFetch = globalThis.fetch;
    let request: Request | undefined;
    globalThis.fetch = async (input, init) => {
      request = input instanceof Request ? input : new Request(input, init);
      return new Response(stream, {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    };
    restore = () => {
      globalThis.fetch = originalFetch;
    };

    const exit = await runPromiseExit(
      gen(function* () {
        const model = yield* Model;
        return yield* model.generate(
          [
            { role: "system", content: "rules" },
            { role: "user", content: "question" },
          ],
          {
            temperature: 0,
            reasoningEffort: "high",
            tools: [
              {
                type: "function",
                function: {
                  name: "lookup",
                  description: "Find a record",
                  parameters: { type: "object" },
                },
              },
            ],
          }
        );
      }).pipe(
        provide(
          makeOpenRouterModelLayer({
            model: "openai/gpt-5",
            apiKey: "sk-test",
            baseUrl: "https://example.test",
          }).pipe(layerProvide(FetchHttpClient.layer))
        )
      )
    );
    assertSuccess(exit);
    expect(request?.url).toBe("https://example.test/api/v1/responses");
    const body = JSON.parse(await request!.clone().text()) as Record<
      string,
      unknown
    >;
    expect(body).toMatchObject({
      input: [
        { type: "message", role: "system", content: "rules" },
        { type: "message", role: "user", content: "question" },
      ],
      stream: true,
      cache_control: { type: "ephemeral" },
      temperature: 0,
    });
    expect(exit.value.completion).toBe("4");
    expect(exit.value.message.toolCalls?.[0]).toEqual({
      id: "call-1",
      type: "function",
      function: { name: "lookup", arguments: '{"x":1}' },
    });
    expect(exit.value.message.responseItems?.at(-1)).toMatchObject({
      call_id: "call-1",
    });
    expect(exit.value.rawResponse).toBeUndefined();
  });

  it("sends trace headers on the OpenRouter request and never overrides auth", async () => {
    const stream = await readFile(
      new URL(
        "../../test/fixtures/advisor-responses-stream.sse",
        import.meta.url
      ),
      "utf8"
    );
    const originalFetch = globalThis.fetch;
    let request: Request | undefined;
    globalThis.fetch = async (input, init) => {
      request = input instanceof Request ? input : new Request(input, init);
      return new Response(stream, {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    };
    restore = () => {
      globalThis.fetch = originalFetch;
    };

    const traceparent =
      "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
    const exit = await runPromiseExit(
      gen(function* () {
        const model = yield* Model;
        return yield* model.generate([{ role: "user", content: "question" }], {
          temperature: 0,
          reasoningEffort: "high",
        });
      }).pipe(
        provide(
          makeOpenRouterModelLayer({
            model: "openai/gpt-5",
            apiKey: "sk-test",
            baseUrl: "https://example.test",
            traceHeaders: {
              traceparent,
              "x-benchmark-trace": "test-trace-key",
              authorization: "Bearer attacker-key",
            },
          }).pipe(layerProvide(FetchHttpClient.layer))
        )
      )
    );
    assertSuccess(exit);
    expect(request?.url).toBe("https://example.test/api/v1/responses");
    expect(request?.headers.get("traceparent")).toBe(traceparent);
    expect(request?.headers.get("x-benchmark-trace")).toBe("test-trace-key");
    expect(request?.headers.get("authorization")).toBe("Bearer sk-test");
  });
});
