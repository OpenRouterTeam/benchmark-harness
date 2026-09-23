import { describe, expect, it } from "bun:test";

import { sandboxAgentCandidateModelsError } from "./candidate-models";

describe("sandboxAgentCandidateModelsError", () => {
  it("returns undefined when no candidate list or algorithm is configured", () => {
    expect(
      sandboxAgentCandidateModelsError({
        benchmarkId: "terminal_bench",
        agent: "pi",
        models: undefined,
        switchyardAlgorithm: undefined,
      })
    ).toBeUndefined();
  });
  it("names the benchmark and agent when a candidate list would be dropped", () => {
    const error = sandboxAgentCandidateModelsError({
      benchmarkId: "terminal_bench",
      agent: "pi",
      models: ["openai/gpt-4.1-nano", "anthropic/claude-sonnet-4.5"],
      switchyardAlgorithm: undefined,
    });
    expect(error?.message).toBe(
      "terminal_bench cannot use candidate models: the pi agent calls the model from inside the sandbox, which does not forward the models list"
    );
  });
  it("names the benchmark and agent when a switchyard algorithm would be dropped", () => {
    const error = sandboxAgentCandidateModelsError({
      benchmarkId: "terminal_bench",
      agent: "pi",
      models: undefined,
      switchyardAlgorithm: "stage",
    });
    expect(error?.message).toBe(
      "terminal_bench cannot use switchyardAlgorithm: the pi agent calls the model from inside the sandbox, which does not forward the switchyard-router plugin"
    );
  });
});
