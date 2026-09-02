import { describe, expect, it } from "bun:test";

import { ResponsesRequest$outboundSchema } from "@openrouter/sdk/models/responsesrequest";

import { MessageRole } from "../harness/core";
import {
  messagesToResponses,
  toolDefinitionToResponses,
} from "./messages-to-responses";
import { toSdkInput } from "./responses-model";

function expectValidResponsesRequest(
  messages: Parameters<typeof messagesToResponses>[0],
  tools?: Parameters<typeof toolDefinitionToResponses>[0][]
) {
  const result = ResponsesRequest$outboundSchema.safeParse({
    model: "openai/gpt-4o-mini",
    input: toSdkInput(messagesToResponses(messages)),
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

  it("validates video content with a processing mode", () => {
    expectValidResponsesRequest([
      {
        role: MessageRole.User,
        content: "",
        contentParts: [
          {
            type: "video_url",
            videoUrl: { url: "video.mp4", processing: "agentic" },
          },
        ],
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
