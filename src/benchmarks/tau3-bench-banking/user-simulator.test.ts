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
  userReasoningEffort: "low",
} as const;

describe("tau3 banking user simulator", () => {
  it("uses Responses function calls and preserves call_id in tool results", async () => {
    const inputs: ChatMessage[][] = [];
    const turns: ResponsesTurn[] = [
      {
        outputItems: [
          {
            type: "reasoning",
            encrypted_content: "opaque",
          },
          {
            type: "function_call",
            call_id: "call-1",
            name: "get_balance",
            arguments: '{"account_id":"a1"}',
          },
        ],
        functionCalls: [
          {
            callId: "call-1",
            name: "get_balance",
            arguments: '{"account_id":"a1"}',
          },
        ],
        text: "",
        generationTimeMs: 1,
      },
      {
        outputItems: [
          { type: "message", content: [{ type: "output_text", text: "Done" }] },
        ],
        functionCalls: [],
        text: "Done",
        generationTimeMs: 1,
      },
    ];
    let index = 0;
    const model: ResponsesModelService = {
      generate: (input, generateConfig) => {
        inputs.push([...input]);
        expect(generateConfig.reasoningEffort).toBe("low");
        return succeed(turns[index++]!);
      },
    };
    const simulator = new UserSimulator(model, config);
    simulator.setAvailableTools([
      {
        type: "function",
        function: {
          name: "get_balance",
          description: undefined,
          parameters: { type: "object" },
        },
      },
    ]);
    simulator.reset("scenario", "Hi");
    const toolTurn = await runPromise(simulator.generateInitial());
    expect(toolTurn).toEqual({
      kind: "toolCalls",
      calls: [
        {
          id: "call-1",
          name: "get_balance",
          arguments: '{"account_id":"a1"}',
        },
      ],
    });
    simulator.addToolResult("call-1", "Balance: $10");
    expect(await runPromise(simulator.continueAfterTools())).toEqual({
      kind: "text",
      content: "Done",
    });
    expect(inputs[1]?.at(-1)).toEqual({
      role: "tool",
      content: "Balance: $10",
      toolCallId: "call-1",
    });
  });
});
