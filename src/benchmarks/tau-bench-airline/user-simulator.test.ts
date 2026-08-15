import { describe, expect, it, spyOn } from "bun:test";
import assert from "node:assert/strict";

import { FetchHttpClient } from "@effect/platform";
import { flatMap, provide, runPromise } from "effect/Effect";

import type { CapturedRequest } from "../../../test/helpers/fetch-sequence";
import { installFetchSequence } from "../../../test/helpers/fetch-sequence";
import { runHarnessPromise } from "../../internal/effect-logger";
import { isRecord } from "../../internal/guards";
import {
  getCollectedGenerationIds,
  resetGenerationIds,
} from "../../runtime/generation-ids";
import { setCurrentEpoch } from "../../runtime/response-cache";
import { UserSimulator } from "./user-simulator";
describe("UserSimulator", () => {
  it.serial(
    "records the generation id from a successful user-model response",
    async () => {
      const originalFetch = globalThis.fetch;
      globalThis.fetch = async () =>
        Response.json({
          id: "tau-user-gen-1",
          choices: [{ message: { content: "Hello" } }],
        });
      try {
        const simulator = new UserSimulator({
          apiKey: "sk-test",
          model: "openai/gpt-4o-mini",
          baseUrl: "https://example.test",
        });
        simulator.reset("scenario", "Hi");
        const ids = await runPromise(
          resetGenerationIds.pipe(
            flatMap(() => simulator.generateInitial()),
            flatMap(() => getCollectedGenerationIds),
            provide(FetchHttpClient.layer)
          )
        );
        expect(ids).toEqual(["tau-user-gen-1"]);
      } finally {
        globalThis.fetch = originalFetch;
      }
    }
  );
  it.serial("sends the cache salt as a header, not a body field", async () => {
    const requests: CapturedRequest[] = [];
    const restore = installFetchSequence(
      [{ id: "tau-user-gen-1", choices: [{ message: { content: "Hello" } }] }],
      requests
    );
    try {
      const simulator = new UserSimulator({
        apiKey: "sk-test",
        model: "openai/gpt-4o-mini",
        baseUrl: "https://example.test",
        sessionId: "wf-123",
      });
      simulator.reset("scenario", "Hi");
      await runPromise(
        setCurrentEpoch(2).pipe(
          flatMap(() => simulator.generateInitial()),
          provide(FetchHttpClient.layer)
        )
      );
      expect(requests[0]?.headers["x-openrouter-cache-salt"]).toBe(
        "wf-123:epoch-2"
      );
      expect(requests[0]?.body["cache_salt"]).toBeUndefined();
    } finally {
      restore();
    }
  });
  it.serial(
    "replays opaque reasoning_details and omits absent details",
    async () => {
      const requests: CapturedRequest[] = [];
      const reasoningDetails = [
        { type: "opaque", payload: { step: 1 } },
        { value: "keep" },
      ];
      const restore = installFetchSequence(
        [
          {
            model: "openai/gpt-4o-mini",
            choices: [
              {
                message: {
                  content: "User turn 1",
                  reasoning_details: reasoningDetails,
                },
              },
            ],
          },
          {
            model: "openai/gpt-4o-mini",
            choices: [{ message: { content: "User turn 2" } }],
          },
          {
            choices: [{ message: { content: "User turn 3" } }],
          },
        ],
        requests
      );
      try {
        const simulator = new UserSimulator({
          apiKey: "sk-test",
          model: "openai/gpt-4o-mini",
          baseUrl: "https://example.test",
        });
        simulator.reset("scenario", "Hi");
        await runPromise(
          simulator.generateInitial().pipe(provide(FetchHttpClient.layer))
        );
        await runPromise(
          simulator.step("Agent reply").pipe(provide(FetchHttpClient.layer))
        );
        await runPromise(
          simulator
            .step("Agent reply again")
            .pipe(provide(FetchHttpClient.layer))
        );
        const secondRequest = requests[1];
        const thirdRequest = requests[2];
        assert(secondRequest);
        assert(thirdRequest);
        const secondMessages = secondRequest.body["messages"];
        const thirdMessages = thirdRequest.body["messages"];
        assert(Array.isArray(secondMessages));
        assert(Array.isArray(thirdMessages));
        const secondAssistant = secondMessages.find(
          (message) => isRecord(message) && message["content"] === "User turn 1"
        );
        const thirdAssistant = thirdMessages.find(
          (message) => isRecord(message) && message["content"] === "User turn 2"
        );
        assert(isRecord(secondAssistant));
        assert(isRecord(thirdAssistant));
        expect(secondAssistant["reasoning_details"]).toEqual(reasoningDetails);
        expect(thirdAssistant["reasoning_details"]).toBeUndefined();
      } finally {
        restore();
      }
    }
  );
  it.serial(
    "replays primary-model details on the fallback-model request",
    async () => {
      const requests: CapturedRequest[] = [];
      const reasoningDetails = [
        { type: "future_variant", payload: { step: 1 } },
      ];
      const restore = installFetchSequence(
        [
          {
            model: "openai/gpt-4o-mini",
            choices: [
              {
                message: {
                  content: "Primary turn",
                  reasoning_details: reasoningDetails,
                },
              },
            ],
          },
          { choices: [{ message: { content: null } }] },
          { choices: [{ message: { content: null } }] },
          { choices: [{ message: { content: null } }] },
          {
            model: "openai/gpt-5.4-mini",
            choices: [{ message: { content: "Fallback turn" } }],
          },
        ],
        requests
      );
      try {
        const simulator = new UserSimulator({
          apiKey: "sk-test",
          model: "openai/gpt-4o-mini",
          baseUrl: "https://example.test",
        });
        simulator.reset("scenario", "Hi");
        await runPromise(
          simulator.generateInitial().pipe(provide(FetchHttpClient.layer))
        );
        await runPromise(
          simulator.step("Agent reply").pipe(provide(FetchHttpClient.layer))
        );
        const fallbackRequest = requests[4];
        assert(fallbackRequest);
        expect(fallbackRequest.body["model"]).toBe("openai/gpt-5.4-mini");
        const messages = fallbackRequest.body["messages"];
        assert(Array.isArray(messages));
        const assistant = messages.find(
          (message) =>
            isRecord(message) && message["content"] === "Primary turn"
        );
        assert(isRecord(assistant));
        expect(assistant["reasoning_details"]).toEqual(reasoningDetails);
      } finally {
        restore();
      }
    }
  );
  it.serial(
    "keeps non-reasoning history in the pre-replay wire shape",
    async () => {
      const requests: CapturedRequest[] = [];
      const restore = installFetchSequence(
        [
          {
            model: "openai/gpt-4o-mini",
            choices: [{ message: { content: "First user turn" } }],
          },
          { choices: [{ message: { content: "Second user turn" } }] },
        ],
        requests
      );
      try {
        const simulator = new UserSimulator({
          apiKey: "sk-test",
          model: "openai/gpt-4o-mini",
          baseUrl: "https://example.test",
        });
        simulator.reset("scenario", "Hi");
        await runPromise(
          simulator.generateInitial().pipe(provide(FetchHttpClient.layer))
        );
        await runPromise(
          simulator.step("Agent reply").pipe(provide(FetchHttpClient.layer))
        );
        const secondRequest = requests[1];
        assert(secondRequest);
        const messages = secondRequest.body["messages"];
        assert(Array.isArray(messages));
        const assistant = messages.find(
          (message) =>
            isRecord(message) && message["content"] === "First user turn"
        );
        assert(isRecord(assistant));
        expect(assistant).toEqual({
          role: "assistant",
          content: "First user turn",
        });
      } finally {
        restore();
      }
    }
  );
  it.serial(
    "retries null message content before returning a recovered response",
    async () => {
      const originalFetch = globalThis.fetch;
      let callCount = 0;
      const responses = [
        { choices: [{ message: { content: null } }] },
        { choices: [{ message: { content: null } }] },
        { choices: [{ message: { content: "Recovered" } }] },
      ];
      globalThis.fetch = async () => {
        const response = responses[callCount] ?? responses.at(-1);
        callCount++;
        return Response.json(response);
      };
      try {
        const simulator = new UserSimulator({
          apiKey: "sk-test",
          model: "openai/gpt-4o-mini",
          baseUrl: "https://example.test",
        });
        simulator.reset("scenario", "Hi");
        const result = await runPromise(
          simulator.generateInitial().pipe(provide(FetchHttpClient.layer))
        );
        expect(result).toBe("Recovered");
        expect(callCount).toBe(3);
      } finally {
        globalThis.fetch = originalFetch;
      }
    }
  );
  it.serial(
    "logs a structured retry warning for each user-sim retry",
    async () => {
      const warn = spyOn(console, "warn").mockImplementation(() => {});
      const originalFetch = globalThis.fetch;
      let callCount = 0;
      const responses = [
        { choices: [{ message: { content: null } }] },
        { choices: [{ message: { content: null } }] },
        { choices: [{ message: { content: "Recovered" } }] },
      ];
      globalThis.fetch = async () => {
        const response = responses[callCount] ?? responses.at(-1);
        callCount++;
        return Response.json(response);
      };
      try {
        const simulator = new UserSimulator({
          apiKey: "sk-test",
          model: "openai/gpt-4o-mini",
          baseUrl: "https://example.test",
        });
        simulator.reset("scenario", "Hi");
        const result = await runHarnessPromise(
          simulator.generateInitial().pipe(provide(FetchHttpClient.layer))
        );
        expect(result).toBe("Recovered");
        expect(warn).toHaveBeenCalledTimes(2);
        expect(warn.mock.calls[0]?.[0]).toBe("Retrying after transient error");
        expect(warn.mock.calls[0]?.[1]).toMatchObject({
          attempt: 1,
          error_tag: "UserSimError",
        });
        expect(warn.mock.calls[1]?.[1]).toMatchObject({ attempt: 2 });
      } finally {
        globalThis.fetch = originalFetch;
        warn.mockRestore();
      }
    }
  );
  it.serial(
    "preserves UserSimError after bounded retries are exhausted",
    async () => {
      const originalFetch = globalThis.fetch;
      let callCount = 0;
      globalThis.fetch = async () => {
        callCount++;
        return Response.json({ choices: [{ message: { content: null } }] });
      };
      try {
        const simulator = new UserSimulator({
          apiKey: "sk-test",
          model: "openai/gpt-4o-mini",
          baseUrl: "https://example.test",
        });
        simulator.reset("scenario", "Hi");
        await expect(
          runPromise(
            simulator.generateInitial().pipe(provide(FetchHttpClient.layer))
          )
        ).rejects.toThrow("User simulator response parse error");
        expect(callCount).toBe(6);
      } finally {
        globalThis.fetch = originalFetch;
      }
    }
  );
});
