import { describe, expect, it } from "bun:test";

import { MessageRole } from "../../harness/core";
import {
  MAX_CONVERSATION_CHARS,
  messageChars,
  selectUserToolDefinitions,
  TerminationReason,
} from "./solver";
describe("bankingSolver", () => {
  it("exports TerminationReason with required values", () => {
    expect(TerminationReason.UserStop).toBe("USER_STOP");
    expect(TerminationReason.MaxSteps).toBe("MAX_STEPS");
    expect(TerminationReason.ConversationTooLarge).toBe(
      "CONVERSATION_TOO_LARGE"
    );
  });
  it("counts content, reasoning and tool call names and arguments", () => {
    expect(
      messageChars({
        role: MessageRole.Assistant,
        content: "abc",
        reasoning: "de",
        toolCalls: [
          {
            id: "call_1",
            type: "function",
            function: { name: "KB_search", arguments: '{"query":"fee"}' },
          },
        ],
      })
    ).toBe(3 + 2 + "KB_search".length + '{"query":"fee"}'.length);
  });
  it("puts a runaway parallel tool-call turn over the conversation cap", () => {
    const searchResult = "x".repeat(30_000);
    const runawayTurnChars =
      4000 * messageChars({ role: MessageRole.Tool, content: searchResult });
    expect(runawayTurnChars).toBeGreaterThan(MAX_CONVERSATION_CHARS);
  });
  it("grants only the user tools declared by the task", () => {
    const definitions = selectUserToolDefinitions([
      "apply_for_credit_card",
      "call_discoverable_user_tool",
    ]);
    expect(definitions.map((definition) => definition.function.name)).toEqual([
      "apply_for_credit_card",
      "call_discoverable_user_tool",
    ]);
  });
});
