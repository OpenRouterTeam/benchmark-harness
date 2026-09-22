import { describe, expect, it } from "bun:test";

import { assertRight } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import { AtifTrajectorySchema } from "./atif-schema";
import { messagesToAtif } from "./messages-to-atif";

describe("messagesToAtif", () => {
  it("returns null for empty messages", () => {
    expect(
      messagesToAtif({ messages: [], model: "openai/gpt-4o-mini" })
    ).toBeNull();
  });

  it("maps system, user, and assistant messages to steps", () => {
    const trajectory = messagesToAtif({
      model: "openai/gpt-4o-mini",
      messages: [
        { role: "system", content: "You are helpful." },
        { role: "user", content: "What is 2+2?" },
        { role: "assistant", content: "4" },
      ],
    });
    expect(trajectory?.steps.map((step) => step.source)).toEqual([
      "system",
      "user",
      "agent",
    ]);
    expect(trajectory?.steps.map((step) => step.message)).toEqual([
      "You are helpful.",
      "What is 2+2?",
      "4",
    ]);
  });

  it("maps tool calls and consecutive tool observations", () => {
    const trajectory = messagesToAtif({
      model: "openai/gpt-4o-mini",
      messages: [
        {
          role: "assistant",
          content: "",
          reasoning: "I should search.",
          toolCalls: [
            {
              id: "call-1",
              type: "function",
              function: { name: "search", arguments: '{"query":"answer"}' },
            },
            {
              id: "call-2",
              type: "function",
              function: { name: "lookup", arguments: '{"id":42}' },
            },
          ],
        },
        { role: "tool", toolCallId: "call-1", content: "first" },
        { role: "tool", toolCallId: "call-2", content: "second" },
      ],
    });
    const step = trajectory?.steps[0];
    expect(step?.reasoning_content).toBe("I should search.");
    expect(step?.tool_calls).toEqual([
      {
        tool_call_id: "call-1",
        function_name: "search",
        arguments: { query: "answer" },
      },
      {
        tool_call_id: "call-2",
        function_name: "lookup",
        arguments: { id: 42 },
      },
    ]);
    expect(step?.observation?.results).toEqual([
      { source_call_id: "call-1", content: "first" },
      { source_call_id: "call-2", content: "second" },
    ]);
  });

  it("preserves an unmatched tool id in result extra", () => {
    const trajectory = messagesToAtif({
      model: "openai/gpt-4o-mini",
      messages: [
        { role: "assistant", content: "Done." },
        { role: "tool", toolCallId: "missing", content: "result" },
      ],
    });
    expect(trajectory?.steps[0]?.observation?.results).toEqual([
      { content: "result", extra: { tool_call_id: "missing" } },
    ]);
  });

  it("uses a raw argument object when arguments are not JSON objects", () => {
    const trajectory = messagesToAtif({
      model: "openai/gpt-4o-mini",
      messages: [
        {
          role: "assistant",
          content: "",
          toolCalls: [
            {
              id: "call-1",
              type: "function",
              function: { name: "search", arguments: "not-json" },
            },
          ],
        },
      ],
    });
    expect(trajectory?.steps[0]?.tool_calls?.[0]?.arguments).toEqual({
      raw: "not-json",
    });
  });

  it("maps supported content parts and records unsupported parts", () => {
    const trajectory = messagesToAtif({
      model: "openai/gpt-4o-mini",
      messages: [
        {
          role: "user",
          content: "fallback",
          contentParts: [
            { type: "text", text: "Look at this." },
            {
              type: "image_url",
              imageUrl: { url: "data:image/png;base64,abc" },
            },
            { type: "video_url", videoUrl: { url: "video.mp4" } },
          ],
        },
      ],
    });
    expect(trajectory?.steps[0]?.message).toEqual([
      { type: "text", text: "Look at this." },
      {
        type: "image",
        source: { media_type: "image/png", path: "data:image/png;base64,abc" },
      },
    ]);
    expect(trajectory?.steps[0]?.extra?.["unsupported_content_parts"]).toEqual([
      { type: "video_url", video_url: { url: "video.mp4" } },
    ]);
  });

  it("maps citations and scorer trajectories into extra", () => {
    const trajectory = messagesToAtif({
      model: "openai/gpt-4o-mini",
      scorerTrajectory: { kind: "verifier_log", log: "passed" },
      messages: [
        {
          role: "user",
          content: "Question",
          citations: [
            {
              url: "https://example.com",
              title: "Example",
              startIndex: 0,
              endIndex: 8,
            },
          ],
        },
      ],
    });
    expect(trajectory?.steps[0]?.extra?.["citations"]).toEqual([
      {
        url: "https://example.com",
        title: "Example",
        start_index: 0,
        end_index: 8,
      },
    ]);
    expect(trajectory?.extra?.["scorer_trajectory"]).toEqual({
      kind: "verifier_log",
      log: "passed",
    });
  });

  it("round-trips through the ATIF schema", () => {
    const trajectory = messagesToAtif({
      model: "openai/gpt-4o-mini",
      messages: [
        { role: "user", content: "Question" },
        { role: "assistant", content: "Answer" },
      ],
    });
    const parsed = parseSchema(AtifTrajectorySchema, trajectory);
    assertRight(parsed);
    expect(parsed.right).toEqual(trajectory);
  });
});
