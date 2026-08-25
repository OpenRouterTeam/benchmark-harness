import { describe, expect, it } from "bun:test";

import type { ChatMessage } from "../harness/core";
import { MessageRole } from "../harness/core";
import {
  chatMessagesToResponses,
  responsesTurnToModelOutput,
  toolDefinitionToResponses,
} from "./chat-to-responses";

describe("chat-to-responses", () => {
  it("maps system and user messages", () => {
    expect(
      chatMessagesToResponses([
        { role: MessageRole.System, content: "rules" },
        { role: MessageRole.User, content: "question" },
      ])
    ).toEqual([
      { type: "message", role: "system", content: "rules" },
      { type: "message", role: "user", content: "question" },
    ]);
  });

  it("maps multimodal content with default and explicit image detail", () => {
    expect(
      chatMessagesToResponses([
        {
          role: MessageRole.User,
          content: "",
          contentParts: [
            { type: "text", text: "look" },
            { type: "image_url", imageUrl: { url: "a.png" } },
            { type: "image_url", imageUrl: { url: "b.png", detail: "high" } },
            { type: "video_url", videoUrl: { url: "v.mp4" } },
          ],
        },
      ])
    ).toEqual([
      {
        type: "message",
        role: "user",
        content: [
          { type: "input_text", text: "look" },
          { type: "input_image", image_url: "a.png", detail: "auto" },
          { type: "input_image", image_url: "b.png", detail: "high" },
          { type: "input_video", video_url: "v.mp4" },
        ],
      },
    ]);
  });

  it("maps assistant tool calls and tool results", () => {
    expect(
      chatMessagesToResponses([
        {
          role: MessageRole.Assistant,
          content: "",
          toolCalls: [
            {
              id: "call-1",
              type: "function",
              function: { name: "lookup", arguments: '{"x":1}' },
            },
          ],
        },
        {
          role: MessageRole.Tool,
          content: "found",
          toolCallId: "call-1",
        },
      ])
    ).toEqual([
      {
        type: "function_call",
        call_id: "call-1",
        name: "lookup",
        arguments: '{"x":1}',
      },
      { type: "function_call_output", call_id: "call-1", output: "found" },
    ]);
  });

  it("replays response items verbatim before synthesized fields", () => {
    const responseItems = [
      { type: "reasoning", encrypted_content: "opaque" },
      { type: "function_call", call_id: "call-2" },
    ];
    const message: ChatMessage = {
      role: MessageRole.Assistant,
      content: "ignored",
      responseItems,
      toolCalls: [
        {
          id: "call-1",
          type: "function",
          function: { name: "ignored", arguments: "{}" },
        },
      ],
    };
    expect(chatMessagesToResponses([message])).toEqual(responseItems);
  });

  it("omits empty assistant content", () => {
    expect(
      chatMessagesToResponses([{ role: MessageRole.Assistant, content: "" }])
    ).toEqual([]);
  });

  it("maps tool definitions and omits undefined descriptions", () => {
    expect(
      toolDefinitionToResponses({
        type: "function",
        function: {
          name: "lookup",
          parameters: { type: "object" },
          strict: true,
        },
      })
    ).toEqual({
      type: "function",
      name: "lookup",
      parameters: { type: "object" },
      strict: true,
    });
  });

  it("uses call_id when converting Responses function calls", () => {
    const output = responsesTurnToModelOutput({
      text: "",
      outputItems: [{ type: "function_call", id: "item-1" }],
      functionCalls: [
        { callId: "call-1", name: "lookup", arguments: '{"x":1}' },
      ],
      generationTimeMs: 7,
    });
    expect(output.message.toolCalls).toEqual([
      {
        id: "call-1",
        type: "function",
        function: { name: "lookup", arguments: '{"x":1}' },
      },
    ]);
  });
});
