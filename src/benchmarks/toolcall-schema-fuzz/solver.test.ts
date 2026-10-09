import { describe, expect, it } from "bun:test";
import assert from "node:assert";

import { HttpClient, HttpClientResponse } from "@effect/platform";
import { provide, runPromiseExit, succeed } from "effect/Effect";
import { isFailure, isSuccess } from "effect/Exit";
import { mergeAll, succeed as layerSucceed } from "effect/Layer";

import { initialTaskState } from "../../harness/core";
import {
  CheckpointStore,
  NOOP_CHECKPOINT_STORE,
  NOOP_PROGRESS_REPORTER,
  ProgressReporter,
} from "../../harness/progress";
import { caseLookup } from "./cases";
import { TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES } from "./realistic/wordings";
import { toolCallSchemaFuzzSolver } from "./solver";

const [ENTRY] = TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES;

function completion(finishReason: "stop" | "length"): Response {
  const body = {
    id: "completion-1",
    object: "chat.completion",
    created: 1,
    model: "test-model",
    choices: [
      {
        index: 0,
        message: { role: "assistant", content: "", refusal: null },
        finish_reason: finishReason,
        native_finish_reason: null,
        logprobs: null,
      },
    ],
  };
  return new Response(JSON.stringify(body), {
    headers: { "content-type": "application/json" },
  });
}

function solve(httpResponse: Response) {
  const entry = ENTRY;
  assert(entry !== undefined);
  const client = HttpClient.make((request) =>
    succeed(HttpClientResponse.fromWeb(request, httpResponse.clone()))
  );
  const solver = toolCallSchemaFuzzSolver(client, {
    options: {
      model: "test-model",
      apiKey: "test-key",
      reasoningEffort: "none",
      sessionId: "session-1",
      retry: { maxRetries: 0 },
    },
    cases: caseLookup([entry]),
  });
  return runPromiseExit(
    solver(
      initialTaskState({
        id: entry.id,
        input: entry.prompt,
        target: { text: "" },
      })
    ).pipe(
      provide(
        mergeAll(
          layerSucceed(ProgressReporter, NOOP_PROGRESS_REPORTER),
          layerSucceed(CheckpointStore, NOOP_CHECKPOINT_STORE)
        )
      )
    )
  );
}

describe("toolCallSchemaFuzzSolver", () => {
  it("records a completed response", async () => {
    const exit = await solve(completion("stop"));
    expect(isFailure(exit)).toBe(false);
  });

  it("records the finish reason for the scorer", async () => {
    const exit = await solve(completion("length"));
    assert(isSuccess(exit));
    expect(exit.value.output?.rawResponse).toStrictEqual({
      finish_reason: "length",
    });
  });
});
