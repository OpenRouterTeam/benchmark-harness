import { describe, expect, it } from "bun:test";

import type { ResponsesRequest, StreamEvents } from "@openrouter/sdk/models";
import type { Effect } from "effect/Effect";
import {
  fail as effectFail,
  succeed as effectSucceed,
  provide,
  runPromise,
  runPromiseExit,
  suspend,
  tryPromise,
} from "effect/Effect";
import type { Exit } from "effect/Exit";
import { isFailure } from "effect/Exit";
import { mergeAll, succeed as layerSucceed } from "effect/Layer";

import {
  noopCheckpointLayer,
  noopProgressLayer,
} from "../../../../test/helpers/noop-progress-layer";
import type { TaskState } from "../../../harness/core";
import { initialTaskState } from "../../../harness/core";
import type {
  AgentStepEvent,
  CheckpointStore,
  ProgressReporter,
} from "../../../harness/progress";
import {
  makeProgressReporter,
  ProgressReporter as ProgressReporterTag,
} from "../../../harness/progress";
import { assertRight } from "../../../internal/testing";
import { parseSchema, z } from "../../../internal/zod";
import type {
  ResponsesResult,
  ResponsesService,
} from "../../../providers/responses-client";
import { ResponsesError } from "../../../providers/responses-client";
import { BENCHMARK_LEAK_EXCLUDED_DOMAINS } from "./blocklist";
import type { SearchLaneConfig } from "./config";
import { SearchLaneConfigSchema } from "./config";
import { DEEP_RESEARCH_INSTRUCTIONS } from "./prompts";
import { SEARCH_SOLVER_METADATA_KEY, searchSolver } from "./solver";

const noopLayers = mergeAll(noopProgressLayer, noopCheckpointLayer);

type SolverEffect = Effect<
  TaskState,
  unknown,
  ProgressReporter | CheckpointStore
>;

function runSolver(effect: SolverEffect): Promise<TaskState> {
  return runPromise(effect.pipe(provide(noopLayers)));
}

function runSolverExit(
  effect: SolverEffect
): Promise<Exit<TaskState, unknown>> {
  return runPromiseExit(effect.pipe(provide(noopLayers)));
}

function makeLane(): SearchLaneConfig {
  const result = parseSchema(SearchLaneConfigSchema, {
    engine: "exa",
    maxAgentTurns: 3,
  });
  assertRight(result);
  return result.right;
}

const LANE = makeLane();

function fixtureResult(overrides: Partial<ResponsesResult>): ResponsesResult {
  return {
    id: "resp-1",
    model: "openai/gpt-5.4-nano",
    status: "completed",
    output: [],
    usage: null,
    text: "",
    generationId: "gen-1",
    provider: "OpenAI",
    generationTimeMs: 0,
    ...overrides,
  };
}

function fixtureService(
  result: ResponsesResult,
  onSend?: (body: ResponsesRequest) => void
): ResponsesService {
  return {
    send: (body) => {
      onSend?.(body);
      return effectSucceed(result);
    },
  };
}

const SolverMetadataSchema = z.object({
  citations: z.array(z.object({ url: z.string(), title: z.string() })),
  responseStatus: z.string().nullable(),
  provider: z.string().nullable(),
  generationId: z.string().nullable(),
});

function readMetadata(metadata: unknown): z.infer<typeof SolverMetadataSchema> {
  const parsed = parseSchema(SolverMetadataSchema, metadata);
  assertRight(parsed);
  return parsed.right;
}
describe("searchSolver", () => {
  it("completes the task with the cleaned answer and captured usage", async () => {
    const result = fixtureResult({
      text: "Exact Answer: 42",
      usage: {
        inputTokens: 100,
        outputTokens: 20,
        totalTokens: 120,
        cost: 0.003,
        outputTokensDetails: { reasoningTokens: 5 },
      },
      output: [
        {
          type: "message",
          content: [
            {
              type: "output_text",
              text: "Exact Answer: 42",
              annotations: [
                {
                  type: "url_citation",
                  url: "https://a.com",
                  title: "A",
                  start_index: 0,
                  end_index: 4,
                },
              ],
            },
          ],
        },
      ],
    });
    const solver = searchSolver(fixtureService(result), {
      model: "openai/gpt-5.4-nano",
      instructions: "Research it.",
      lane: LANE,
    });
    const state = await runSolver(
      solver(
        initialTaskState({
          id: "browsecomp-0",
          input: "Who?",
          target: { text: "42" },
        })
      )
    );
    expect(state.completed).toBe(true);
    expect(state.output?.completion).toBe("Exact Answer: 42");
    expect(state.output?.usage).toEqual({
      inputTokens: 100,
      outputTokens: 20,
      totalTokens: 120,
      reasoningTokens: 5,
      totalCost: 0.003,
    });
    const meta = readMetadata(
      state.sample.metadata?.[SEARCH_SOLVER_METADATA_KEY]
    );
    expect(meta.citations).toEqual([{ url: "https://a.com", title: "A" }]);
    expect(meta.provider).toBe("OpenAI");
  });
  it("uses one call when deep research already contains an Exact Answer", async () => {
    let calls = 0;
    const service: ResponsesService = {
      send: () => {
        calls += 1;
        return effectSucceed(
          fixtureResult({ text: "Explanation: solved.\nExact Answer: 42" })
        );
      },
    };
    const solver = searchSolver(service, {
      model: "m",
      instructions: DEEP_RESEARCH_INSTRUCTIONS,
      lane: LANE,
    });
    const state = await runSolver(
      solver(
        initialTaskState({ id: "s", input: "Who?", target: { text: "42" } })
      )
    );
    expect(calls).toBe(1);
    expect(state.output?.completion).toContain("Exact Answer: 42");
  });
  it("records the one-call request input and raw output items in order", async () => {
    const output = [
      {
        type: "web_search_call",
        id: "search-1",
        status: "completed",
        action: { type: "search", query: "Who?" },
      },
      {
        type: "message",
        id: "message-1",
        content: [
          { type: "output_text", text: "Exact Answer: 42", annotations: [] },
        ],
      },
    ];
    const solver = searchSolver(
      fixtureService(fixtureResult({ text: "Exact Answer: 42", output })),
      {
        model: "m",
        instructions: DEEP_RESEARCH_INSTRUCTIONS,
        lane: LANE,
      }
    );
    const state = await runSolver(
      solver(
        initialTaskState({ id: "s", input: "Who?", target: { text: "42" } })
      )
    );
    expect(state.responseItems).toEqual([
      { role: "user", content: "Who?" },
      ...output,
    ]);
  });
  it("sends the lane surface on the request body", async () => {
    let sent: ResponsesRequest | undefined;
    const solver = searchSolver(
      fixtureService(fixtureResult({ text: "x" }), (body) => {
        sent = body;
      }),
      { model: "m", instructions: "i", lane: LANE }
    );
    const state = await runSolver(
      solver(initialTaskState({ id: "s", input: "q", target: { text: "t" } }))
    );
    expect(sent?.maxToolCalls).toBe(3);
    expect(sent?.tools).toEqual([
      {
        type: "openrouter:web_search",
        parameters: {
          engine: "exa",
          excludedDomains: [...BENCHMARK_LEAK_EXCLUDED_DOMAINS],
        },
      },
    ]);
    expect(state.requestBody).toEqual(sent);
  });
  it("trims whitespace from the answer", async () => {
    const solver = searchSolver(
      fixtureService(fixtureResult({ text: "  Exact Answer: 7\n" })),
      {
        model: "m",
        instructions: "i",
        lane: LANE,
      }
    );
    const state = await runSolver(
      solver(initialTaskState({ id: "s", input: "q", target: { text: "t" } }))
    );
    expect(state.output?.completion).toBe("Exact Answer: 7");
  });
  it("reports stream progress events through ProgressReporter.onAgentStep", async () => {
    const streamEvents: StreamEvents[] = [
      {
        type: "response.output_item.added",
        outputIndex: 0,
        sequenceNumber: 1,
        item: { type: "web_search_call", id: "ws-1", status: "in_progress" },
      },
      {
        type: "response.output_item.done",
        outputIndex: 0,
        sequenceNumber: 2,
        item: {
          type: "web_search_call",
          id: "ws-1",
          status: "completed",
          action: { type: "search", query: "who?" },
        },
      },
    ];
    const service: ResponsesService = {
      send: (_body, options) => {
        for (const event of streamEvents) {
          options.onStreamEvent?.(event);
        }
        return effectSucceed(fixtureResult({ text: "Exact Answer: 42" }));
      },
    };
    const reported: {
      event: AgentStepEvent;
      sampleId: string;
      epoch: number;
    }[] = [];
    const recordingLayer = layerSucceed(
      ProgressReporterTag,
      makeProgressReporter({
        onAgentStep: (event, sampleId, epoch) => {
          reported.push({ event, sampleId, epoch });
        },
      })
    );
    const solver = searchSolver(service, {
      model: "m",
      instructions: "i",
      lane: LANE,
    });
    await runPromise(
      solver({
        ...initialTaskState({ id: "s", input: "q", target: { text: "t" } }),
        epoch: 2,
      }).pipe(provide(mergeAll(recordingLayer, noopCheckpointLayer)))
    );
    expect(reported).toEqual([
      {
        event: { type: "turn", step: 0, toolCallIndex: 0 },
        sampleId: "s",
        epoch: 2,
      },
      {
        event: {
          type: "tool-call",
          step: 0,
          toolCallIndex: 1,
          command: "who?",
        },
        sampleId: "s",
        epoch: 2,
      },
    ]);
  });
  it("maps ResponsesError to a retryable ModelError", async () => {
    const service: ResponsesService = {
      send: () =>
        effectFail(new ResponsesError({ message: "boom", retryable: true })),
    };
    const solver = searchSolver(service, {
      model: "m",
      instructions: "i",
      lane: LANE,
      retry: { maxRetries: 0 },
    });
    const exit = await runSolverExit(
      solver(initialTaskState({ id: "s", input: "q", target: { text: "t" } }))
    );
    expect(isFailure(exit)).toBe(true);
  });
  it("retries transient generation errors up to the configured maxRetries", async () => {
    let attempts = 0;
    const service: ResponsesService = {
      send: () =>
        suspend(() => {
          attempts += 1;
          return attempts < 3
            ? effectFail(
                new ResponsesError({ message: "boom", retryable: true })
              )
            : effectSucceed(fixtureResult({ text: "Exact Answer: 42" }));
        }),
    };
    const solver = searchSolver(service, {
      model: "m",
      instructions: "i",
      lane: LANE,
      retry: { maxRetries: 3, baseDelayMs: 1 },
    });
    const state = await runSolver(
      solver(initialTaskState({ id: "s", input: "q", target: { text: "t" } }))
    );
    expect(attempts).toBe(3);
    expect(state.output?.completion).toBe("Exact Answer: 42");
  });
  it("retries non-completed and empty terminal responses", async () => {
    let attempts = 0;
    const service: ResponsesService = {
      send: () =>
        suspend(() => {
          attempts += 1;
          if (attempts === 1) {
            return effectSucceed(
              fixtureResult({ status: "incomplete", text: "partial" })
            );
          }
          if (attempts === 2) {
            return effectSucceed(fixtureResult({ text: "   " }));
          }
          return effectSucceed(fixtureResult({ text: "Exact Answer: 42" }));
        }),
    };
    const solver = searchSolver(service, {
      model: "m",
      instructions: "i",
      lane: LANE,
      retry: { maxRetries: 3, baseDelayMs: 1 },
    });
    const state = await runSolver(
      solver(initialTaskState({ id: "s", input: "q", target: { text: "t" } }))
    );
    expect(attempts).toBe(3);
    expect(state.output?.completion).toBe("Exact Answer: 42");
  });
  it("returns the last completed blank response after retries are exhausted", async () => {
    let attempts = 0;
    const service: ResponsesService = {
      send: () => {
        attempts += 1;
        return effectSucceed(
          fixtureResult({
            text: "   ",
            usage: {
              inputTokens: 10,
              outputTokens: 0,
              totalTokens: 10,
              cost: 0.01,
            },
          })
        );
      },
    };
    const solver = searchSolver(service, {
      model: "m",
      instructions: "i",
      lane: LANE,
      retry: { maxRetries: 2, baseDelayMs: 1 },
    });
    const state = await runSolver(
      solver(initialTaskState({ id: "s", input: "q", target: { text: "t" } }))
    );
    expect(attempts).toBe(3);
    expect(state.output?.completion).toBe("");
    expect(state.output?.usage?.totalCost).toBe(0.01);
  });
  it("aborts and retries an attempt that stalls mid-stream", async () => {
    let attempts = 0;
    let aborts = 0;
    const service: ResponsesService = {
      send: (_body, options) =>
        tryPromise({
          try: (signal) => {
            attempts += 1;
            if (attempts > 1) {
              return Promise.resolve(
                fixtureResult({ text: "Exact Answer: 42" })
              );
            }
            options.onStreamEvent?.({
              type: "response.output_item.added",
              outputIndex: 0,
              sequenceNumber: 1,
              item: { type: "reasoning", id: "r-1", summary: [] },
            });
            const stalled = Promise.withResolvers<ResponsesResult>();
            signal.addEventListener(
              "abort",
              () => {
                aborts += 1;
                stalled.reject(signal.reason);
              },
              { once: true }
            );
            return stalled.promise;
          },
          catch: (cause) =>
            new ResponsesError({
              message: String(cause),
              retryable: true,
            }),
        }),
    };
    const solver = searchSolver(service, {
      model: "m",
      instructions: "i",
      lane: LANE,
      timeoutMs: 25,
      retry: { maxRetries: 1, baseDelayMs: 1 },
    });
    const state = await runSolver(
      solver({
        ...initialTaskState({ id: "s", input: "q", target: { text: "t" } }),
        epoch: 0,
      })
    );
    expect(attempts).toBe(2);
    expect(aborts).toBe(1);
    expect(state.output?.completion).toBe("Exact Answer: 42");
  });
  it("does not retry past maxRetries", async () => {
    let attempts = 0;
    const service: ResponsesService = {
      send: () =>
        suspend(() => {
          attempts += 1;
          return effectFail(
            new ResponsesError({ message: "boom", retryable: true })
          );
        }),
    };
    const solver = searchSolver(service, {
      model: "m",
      instructions: "i",
      lane: LANE,
      retry: { maxRetries: 1, baseDelayMs: 1 },
    });
    const exit = await runSolverExit(
      solver(initialTaskState({ id: "s", input: "q", target: { text: "t" } }))
    );
    expect(isFailure(exit)).toBe(true);
    expect(attempts).toBe(2);
  });
});
