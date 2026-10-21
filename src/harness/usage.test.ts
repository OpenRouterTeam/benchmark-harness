import { describe, expect, it } from "bun:test";

import { mergeModelUsages } from "./usage";
describe("mergeModelUsages", () => {
  it("sums every numeric usage and server-tool field across N calls", () => {
    expect(
      mergeModelUsages([
        {
          inputTokens: 10,
          outputTokens: 5,
          totalTokens: 15,
          reasoningTokens: 2,
          totalCost: 0.02,
          serverToolUse: {
            webSearchRequests: 2,
            toolCallsRequested: 3,
            toolCallsExecuted: 2,
          },
        },
        undefined,
        {
          inputTokens: 4,
          outputTokens: 2,
          totalTokens: 6,
          reasoningTokens: 1,
          totalCost: 0.001,
          serverToolUse: {
            webSearchRequests: 1,
            toolCallsRequested: 2,
            toolCallsExecuted: 1,
          },
        },
        { totalTokens: 3, serverToolUse: { toolCallsExecuted: 4 } },
      ])
    ).toEqual({
      inputTokens: 14,
      outputTokens: 7,
      totalTokens: 24,
      reasoningTokens: 3,
      totalCost: 0.021,
      serverToolUse: {
        webSearchRequests: 3,
        toolCallsRequested: 5,
        toolCallsExecuted: 7,
      },
    });
  });
  it("returns undefined when no call reports usage", () => {
    expect(mergeModelUsages([undefined, undefined])).toBeUndefined();
  });
  it("preserves the available object when exactly one call reports usage", () => {
    const usage = { totalTokens: 3, serverToolUse: { webSearchRequests: 1 } };
    expect(mergeModelUsages([undefined, usage])).toBe(usage);
  });
});
