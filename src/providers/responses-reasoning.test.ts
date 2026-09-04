import { describe, expect, it } from "bun:test";

import { reasoningFromOutputItems } from "./responses-reasoning";

describe("reasoningFromOutputItems", () => {
  it("normalizes plaintext reasoning content into readable reasoning and text details", () => {
    expect(
      reasoningFromOutputItems([
        {
          type: "reasoning",
          id: "rs_1",
          content: [
            { type: "reasoning_text", text: "1. Analyze the request" },
            { type: "reasoning_text", text: "2. Answer" },
          ],
          summary: [],
        },
        { type: "message", content: [{ type: "output_text", text: "B" }] },
      ])
    ).toEqual({
      reasoning: "1. Analyze the request\n\n2. Answer",
      reasoningDetails: [
        {
          type: "reasoning.text",
          text: "1. Analyze the request",
          id: "rs_1",
        },
        { type: "reasoning.text", text: "2. Answer", id: "rs_1" },
      ],
    });
  });

  it("carries the signature and format of a plaintext item", () => {
    expect(
      reasoningFromOutputItems([
        {
          type: "reasoning",
          id: "rs_1",
          format: "anthropic-claude-v1",
          signature: "sig",
          content: [{ type: "reasoning_text", text: "thought" }],
        },
      ])
    ).toEqual({
      reasoning: "thought",
      reasoningDetails: [
        {
          type: "reasoning.text",
          text: "thought",
          id: "rs_1",
          format: "anthropic-claude-v1",
          signature: "sig",
        },
      ],
    });
  });

  it("keeps an encrypted blob as a detail and emits no readable reasoning", () => {
    expect(
      reasoningFromOutputItems([
        {
          type: "reasoning",
          id: "rs_2",
          encrypted_content: "gAAAAAopaque",
          summary: [],
        },
      ])
    ).toEqual({
      reasoningDetails: [
        { type: "reasoning.encrypted", data: "gAAAAAopaque", id: "rs_2" },
      ],
    });
  });

  it("falls back to provider summaries when no plaintext is exposed", () => {
    expect(
      reasoningFromOutputItems([
        {
          type: "reasoning",
          id: "rs_3",
          summary: [
            { type: "summary_text", text: "Considered two options" },
            { type: "summary_text", text: "Picked the second" },
          ],
          encrypted_content: "blob",
        },
      ])
    ).toEqual({
      reasoning: "Considered two options\n\nPicked the second",
      reasoningDetails: [
        {
          type: "reasoning.summary",
          summary: "Considered two options",
          id: "rs_3",
        },
        {
          type: "reasoning.summary",
          summary: "Picked the second",
          id: "rs_3",
        },
        { type: "reasoning.encrypted", data: "blob", id: "rs_3" },
      ],
    });
  });

  it("prefers plaintext over summaries for readable reasoning while keeping both details", () => {
    const result = reasoningFromOutputItems([
      {
        type: "reasoning",
        content: [{ type: "reasoning_text", text: "raw thought" }],
        summary: [{ type: "summary_text", text: "short summary" }],
      },
    ]);
    expect(result.reasoning).toBe("raw thought");
    expect(result.reasoningDetails).toEqual([
      { type: "reasoning.text", text: "raw thought" },
      { type: "reasoning.summary", summary: "short summary" },
    ]);
  });

  it("joins plaintext across multiple reasoning items in wire order", () => {
    expect(
      reasoningFromOutputItems([
        {
          type: "reasoning",
          content: [{ type: "reasoning_text", text: "first" }],
        },
        { type: "function_call", call_id: "c1", name: "t", arguments: "{}" },
        {
          type: "reasoning",
          content: [{ type: "reasoning_text", text: "second" }],
        },
      ]).reasoning
    ).toBe("first\n\nsecond");
  });

  it("returns nothing when no reasoning item is present", () => {
    expect(
      reasoningFromOutputItems([
        { type: "message", content: [{ type: "output_text", text: "B" }] },
      ])
    ).toEqual({});
  });

  it("returns nothing for a reasoning item carrying no reasoning at all", () => {
    expect(
      reasoningFromOutputItems([
        { type: "reasoning", id: "rs_4", summary: [], content: [] },
      ])
    ).toEqual({});
  });

  it("skips malformed parts rather than repairing them", () => {
    expect(
      reasoningFromOutputItems([
        {
          type: "reasoning",
          content: [
            "not-an-object",
            { type: "reasoning_text" },
            { type: "reasoning_text", text: "" },
            { type: "reasoning_text", text: 42 },
            { type: "reasoning_text", text: "kept" },
          ],
          summary: "not-an-array",
          encrypted_content: 7,
        },
      ])
    ).toEqual({
      reasoning: "kept",
      reasoningDetails: [{ type: "reasoning.text", text: "kept" }],
    });
  });
});
