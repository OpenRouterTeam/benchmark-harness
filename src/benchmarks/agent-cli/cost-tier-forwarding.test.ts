import { describe, expect, it } from "bun:test";

import { FetchHttpClient } from "@effect/platform";
import { failureOption } from "effect/Cause";
import { provide, runPromiseExit, scoped } from "effect/Effect";
import { build } from "effect/Layer";
import { getOrThrow } from "effect/Option";

import { assertFailure } from "../../../test/helpers/exit-asserts";
import {
  DeepSweConfigSchema,
  SweAtlasQaConfigSchema,
} from "../benchmark-config";
import { DEEP_SWE_BENCHMARK } from "../deep-swe/benchmark";
import { SWE_ATLAS_QA_BENCHMARK } from "../swe-atlas/benchmark";

describe.each([
  ["openrouter/auto", "auto-router"],
  ["openrouter/auto-beta", "auto-beta-router"],
  ["typesafe/jev-router", "jev-router"],
] as const)("coding benchmark %s tiers", (model, pluginId) => {
  it.each([
    {
      id: "deep_swe",
      schema: DeepSweConfigSchema,
      benchmark: DEEP_SWE_BENCHMARK,
    },
    {
      id: "swe_atlas_qa",
      schema: SweAtlasQaConfigSchema,
      benchmark: SWE_ATLAS_QA_BENCHMARK,
    },
  ] as const)(
    "refuses an unforwardable tier before starting $id",
    async ({ id, schema, benchmark }) => {
      const exit = await runPromiseExit(
        scoped(
          build(
            benchmark.makeLayer({
              apiKey: "sk-test",
              sessionId: "run-1",
              benchmarkConfig: schema.parse({
                benchmarkId: id,
                model,
                reasoningEffort: "auto",
                agentReasoningEffort: "high",
                agent: "claude",
                costTier: "high",
              }),
            })
          )
        ).pipe(provide(FetchHttpClient.layer))
      );
      assertFailure(exit);
      expect(getOrThrow(failureOption(exit.cause)).message).toBe(
        `${id} cannot use costTier: the claude agent calls the model from inside the sandbox and does not forward the ${pluginId} plugin (supported agents: pi)`
      );
    }
  );
});
