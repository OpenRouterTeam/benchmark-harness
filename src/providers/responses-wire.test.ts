import { describe, expect, it } from "bun:test";

import { ResponsesRequest$outboundSchema } from "@openrouter/sdk/models/responsesrequest";
import { camelCase } from "change-case";

import { MessageRole } from "../harness/core";
import {
  messagesToResponses,
  toolDefinitionToResponses,
} from "./messages-to-responses";

const RAW_PAYLOAD_KEYS = new Set(["arguments", "output"]);

function toSdkValue(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(toSdkValue);
  }
  if (value === null || typeof value !== "object") {
    return value;
  }
  return Object.fromEntries(
    Object.entries(value).map(([key, nestedValue]) => [
      camelCase(key),
      RAW_PAYLOAD_KEYS.has(key) ? nestedValue : toSdkValue(nestedValue),
    ])
  );
}

function expectValidResponsesRequest(
  messages: Parameters<typeof messagesToResponses>[0],
  tools?: Parameters<typeof toolDefinitionToResponses>[0][]
) {
  const result = ResponsesRequest$outboundSchema.safeParse({
    model: "openai/gpt-4o-mini",
    input: toSdkValue(messagesToResponses(messages)),
    store: false,
    stream: true,
    serviceTier: null,
    ...(tools !== undefined && {
      tools: tools.map(toolDefinitionToResponses),
    }),
  });
  expect(result.success).toBe(true);
}

describe("Responses wire contract", () => {
  it("validates text-only system and user messages", () => {
    expectValidResponsesRequest([
      { role: MessageRole.System, content: "Follow the rules." },
      { role: MessageRole.User, content: "Answer the question." },
    ]);
  });

  it("validates vision messages with default and explicit detail", () => {
    expectValidResponsesRequest([
      {
        role: MessageRole.User,
        content: "",
        contentParts: [
          { type: "image_url", imageUrl: { url: "without-detail.png" } },
          {
            type: "image_url",
            imageUrl: { url: "high-detail.png", detail: "high" },
          },
        ],
      },
    ]);
  });

  it("validates video content", () => {
    expectValidResponsesRequest([
      {
        role: MessageRole.User,
        content: "",
        contentParts: [{ type: "video_url", videoUrl: { url: "video.mp4" } }],
      },
    ]);
  });

  it("validates an assistant function call and its output", () => {
    expectValidResponsesRequest(
      [
        { role: MessageRole.System, content: "You are helpful." },
        { role: MessageRole.User, content: "Look up my balance." },
        {
          role: MessageRole.Assistant,
          content: "",
          toolCalls: [
            {
              id: "call-1",
              type: "function",
              function: {
                name: "get_balance",
                arguments: '{"account_id":"acct-1"}',
              },
            },
          ],
        },
        {
          role: MessageRole.Tool,
          content: '{"balance":100}',
          toolCallId: "call-1",
        },
      ],
      [
        {
          type: "function",
          function: {
            name: "get_balance",
            description: "Get the current balance.",
            parameters: {
              type: "object",
              properties: { account_id: { type: "string" } },
              required: ["account_id"],
              additionalProperties: false,
            },
            strict: true,
          },
        },
      ]
    );
  });
});
