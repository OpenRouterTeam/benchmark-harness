import { describe, expect, it } from "bun:test";

import { runPromise, succeed } from "effect/Effect";

import type { ChatMessage } from "../../harness/core";
import type {
  ResponsesModelService,
  ResponsesTurn,
} from "../../providers/responses-model";
import { UserSimulator } from "./user-simulator";

const config = {
  apiKey: "sk-test",
  model: "openai/gpt-5",
  sessionId: "session-1",
} as const;

function modelFor(
  turns: readonly ResponsesTurn[],
  inputs: ChatMessage[][]
): ResponsesModelService {
  let index = 0;
  return {
    generate: (input) => {
      inputs.push([...input]);
      return succeed(turns[Math.min(index++, turns.length - 1)]!);
    },
  };
}

describe("tau-bench airline user simulator", () => {
  it("uses Responses turns and replays output items", async () => {
    const inputs: ChatMessage[][] = [];
    const responseItems = [
      { type: "reasoning", encrypted_content: "opaque" },
      { type: "message", content: [{ type: "output_text", text: "Hello" }] },
    ];
    const model = modelFor(
      [
        {
          outputItems: responseItems,
          functionCalls: [],
          text: "Hello",
          generationTimeMs: 1,
        },
        {
          outputItems: [],
          functionCalls: [],
          text: "Goodbye",
          generationTimeMs: 1,
        },
      ],
      inputs
    );
    const simulator = new UserSimulator(model, config);
    simulator.reset("scenario", "Hi");
    expect(await runPromise(simulator.generateInitial())).toBe("Hello");
    expect(await runPromise(simulator.step("How are you?"))).toBe("Goodbye");
    expect(inputs[1]).toEqual([
      { role: "system", content: expect.any(String) },
      { role: "user", content: "Hi" },
      {
        role: "assistant",
        content: "Hello",
        responseItems,
      },
      { role: "user", content: "How are you?" },
    ]);
  });
});
