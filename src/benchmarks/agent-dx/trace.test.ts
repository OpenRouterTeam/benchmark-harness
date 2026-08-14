import { describe, expect, it } from "bun:test";

import type { BenchmarkResultRow } from "../../results/parquet-schema";
import {
  agentEventStreamFromMessages,
  formatTrialTrace,
  frictionFromEvents,
  parseSubchecks,
  parseTraceEvents,
  resourceUsageFromEvents,
  tracesFromResultRows,
} from "./trace";

const EVENT_STREAM = [
  JSON.stringify({
    type: "text",
    part: { text: "I will default to google/gemini-2.0-flash-001." },
  }),
  JSON.stringify({
    type: "reasoning",
    part: { text: "The docs snapshot shows the request shape." },
  }),
  JSON.stringify({
    type: "tool_use",
    part: {
      type: "tool",
      tool: "grep",
      state: {
        status: "completed",
        input: { pattern: "image_url", path: "/opt/openrouter-docs" },
        output: "Found 100 matches",
      },
    },
  }),
  JSON.stringify({
    type: "tool_use",
    tool: "webfetch",
    state: {
      status: "completed",
      input: { url: "https://openrouter.ai/docs" },
      output: "docs",
    },
  }),
  "not json",
  JSON.stringify({ type: "step_finish", part: { tokens: { input: 1 } } }),
].join("\n");

function sampleRow(overrides: Partial<BenchmarkResultRow>): BenchmarkResultRow {
  return {
    format_version: 1,
    task: "agent_dx",
    model: "openai/gpt-5.6-luna",
    epochs: 1,
    temperature: 0,
    benchmark_config: null,
    created_at: "2026-07-27T00:00:00Z",
    accuracy: 0,
    total_questions: 1,
    correct_answers: 0,
    input_tokens: 0,
    output_tokens: 0,
    total_tokens: 0,
    reasoning_tokens: 0,
    total_cost: 0,
    generation_time_ms: 0,
    epoch_total_questions: null,
    epoch_correct_answers: null,
    extra_scores: null,
    primary_score: null,
    sample_id: "agent_dx-image-input",
    epoch: 0,
    input: null,
    target: null,
    score_value: "I",
    answer: null,
    explanation: null,
    scorer_trajectory: null,
    response_items: null,
    generation_ids: null,
    messages: null,
    metadata: null,
    ...overrides,
  };
}

describe("parseTraceEvents", () => {
  it("extracts text, reasoning, and tool events and skips malformed lines", () => {
    const events = parseTraceEvents(EVENT_STREAM);
    expect(events).toEqual([
      { kind: "text", text: "I will default to google/gemini-2.0-flash-001." },
      { kind: "reasoning", text: "The docs snapshot shows the request shape." },
      {
        kind: "tool",
        tool: "grep",
        input: JSON.stringify({
          pattern: "image_url",
          path: "/opt/openrouter-docs",
        }),
        outputPreview: "Found 100 matches",
        errored: false,
      },
      {
        kind: "tool",
        tool: "webfetch",
        input: JSON.stringify({ url: "https://openrouter.ai/docs" }),
        outputPreview: "docs",
        errored: false,
      },
    ]);
  });
});

describe("frictionFromEvents", () => {
  const tool = (
    name: string,
    input: Record<string, string> = {},
    errored = false
  ) =>
    ({
      kind: "tool",
      tool: name,
      input: JSON.stringify(input),
      outputPreview: "",
      errored,
    }) as const;

  it("counts tool calls, errored calls, and app-run retries", () => {
    const friction = frictionFromEvents([
      { kind: "text", text: "thinking" },
      tool("bash", { command: "npm install" }),
      tool("bash", { command: "npm start" }, true),
      tool("edit", { filePath: "/app/src/index.ts" }),
      tool("bash", { command: "npm start" }),
      tool("bash", { command: "npx tsc --noEmit" }),
    ]);
    expect(friction).toEqual({
      toolCalls: 5,
      erroredToolCalls: 1,
      appRunRetries: 2,
    });
  });

  it("reports zero retries for a single app run and marks error states", () => {
    const events = parseTraceEvents(
      JSON.stringify({
        type: "tool_use",
        part: {
          type: "tool",
          tool: "bash",
          state: {
            status: "error",
            input: { command: "npm start" },
            output: "crashed",
          },
        },
      })
    );
    expect(frictionFromEvents(events)).toEqual({
      toolCalls: 1,
      erroredToolCalls: 1,
      appRunRetries: 0,
    });
  });
});

describe("parseTraceEvents (claude-code stream-json)", () => {
  it("extracts content blocks from assistant events", () => {
    const stream = [
      JSON.stringify({
        type: "assistant",
        message: {
          content: [
            { type: "thinking", thinking: "Check live model data first." },
            { type: "text", text: "Querying the OpenRouter MCP." },
            {
              type: "tool_use",
              name: "mcp__openrouter__search-models",
              input: { query: "image input" },
            },
          ],
        },
      }),
      JSON.stringify({
        type: "result",
        subtype: "success",
        usage: { input_tokens: 1 },
      }),
    ].join("\n");
    const events = parseTraceEvents(stream);
    expect(events).toEqual([
      { kind: "reasoning", text: "Check live model data first." },
      { kind: "text", text: "Querying the OpenRouter MCP." },
      {
        kind: "tool",
        tool: "mcp__openrouter__search-models",
        input: JSON.stringify({ query: "image input" }),
        outputPreview: "",
      },
    ]);
    expect(resourceUsageFromEvents(events).mcpToolCalls).toBe(1);
  });

  it("marks tool events errored from tool_result blocks in user events", () => {
    const stream = [
      JSON.stringify({
        type: "assistant",
        message: {
          content: [
            {
              type: "tool_use",
              id: "toolu_ok",
              name: "Bash",
              input: { command: "bun run start" },
            },
            {
              type: "tool_use",
              id: "toolu_bad",
              name: "Bash",
              input: { command: "bun run test" },
            },
          ],
        },
      }),
      JSON.stringify({
        type: "user",
        message: {
          content: [
            { type: "tool_result", tool_use_id: "toolu_ok", content: "done" },
            {
              type: "tool_result",
              tool_use_id: "toolu_bad",
              is_error: true,
              content: [{ type: "text", text: "command failed: exit 1" }],
            },
            { type: "tool_result", tool_use_id: "toolu_unknown" },
          ],
        },
      }),
    ].join("\n");
    const events = parseTraceEvents(stream);
    expect(events).toEqual([
      {
        kind: "tool",
        tool: "Bash",
        input: JSON.stringify({ command: "bun run start" }),
        outputPreview: "done",
        errored: false,
      },
      {
        kind: "tool",
        tool: "Bash",
        input: JSON.stringify({ command: "bun run test" }),
        outputPreview: "command failed: exit 1",
        errored: true,
      },
    ]);
    expect(frictionFromEvents(events).erroredToolCalls).toBe(1);
  });
});

describe("parseSubchecks", () => {
  it("parses SUBCHECK lines from verifier output", () => {
    const output = [
      "SUBCHECK project_present=pass",
      "SUBCHECK app_ran=pass",
      "SUBCHECK verified=fail",
      "VERIFY FAIL: generation used a dead model",
    ].join("\n");
    expect(parseSubchecks(output)).toEqual({
      project_present: true,
      app_ran: true,
      verified: false,
    });
  });

  it("returns empty for output without subchecks", () => {
    expect(parseSubchecks("VERIFY PASS")).toEqual({});
  });
});

describe("resourceUsageFromEvents", () => {
  it("counts MCP, skill, docs, and webfetch invocations", () => {
    const tool = (name: string, input: Record<string, string> = {}) =>
      ({
        kind: "tool",
        tool: name,
        input: JSON.stringify(input),
        outputPreview: "",
      }) as const;
    const usage = resourceUsageFromEvents([
      { kind: "text", text: "thinking" },
      tool("openrouter_search-models", { query: "image generation" }),
      tool("openrouter_get-model"),
      tool("skill", { name: "openrouter-models" }),
      tool("webfetch", { url: "https://openrouter.ai/docs" }),
      tool("grep", { pattern: "image", path: "/opt/openrouter-docs" }),
      tool("read", { filePath: "/opt/openrouter-docs/llms-full.txt" }),
      tool("bash", { command: "npm install" }),
    ]);
    expect(usage).toEqual({
      mcpToolCalls: 2,
      skillInvocations: 1,
      docsReads: 2,
      webFetches: 1,
    });
  });

  it("counts shell curl/wget fetches as web fetches", () => {
    const tool = (name: string, input: Record<string, string> = {}) =>
      ({
        kind: "tool",
        tool: name,
        input: JSON.stringify(input),
        outputPreview: "",
      }) as const;
    const usage = resourceUsageFromEvents([
      tool("bash", {
        command: "curl -s https://openrouter.ai/docs/quickstart",
      }),
      tool("shell", {
        command: "wget -qO- https://openrouter.ai/api/v1/models",
      }),
      tool("bash", { command: "curl --version" }),
      tool("bash", { command: "npm install" }),
    ]);
    expect(usage.webFetches).toBe(2);
  });

  it("counts Claude Code's capitalized Read of a SKILL.md as a skill invocation", () => {
    const usage = resourceUsageFromEvents([
      {
        kind: "tool",
        tool: "Read",
        input: JSON.stringify({
          file_path: "/root/.claude/skills/openrouter-models/SKILL.md",
        }),
        outputPreview: "",
      },
    ]);
    expect(usage.skillInvocations).toBe(1);
  });
});

describe("tracesFromResultRows", () => {
  it("builds trial traces from result rows", () => {
    const messages = JSON.stringify([
      { role: "user", content: "instruction" },
      { role: "assistant", content: EVENT_STREAM },
    ]);
    const row = sampleRow({
      messages,
      explanation:
        "SUBCHECK app_ran=pass\nSUBCHECK verified=fail\nVERIFY FAIL: dead model",
    });

    const [trace] = tracesFromResultRows([row]);
    expect(trace?.taskId).toBe("agent_dx-image-input");
    expect(trace?.passed).toBe(false);
    expect(trace?.events).toHaveLength(4);
    expect(trace?.subchecks).toEqual({ app_ran: true, verified: false });

    const report = formatTrialTrace(trace!);
    expect(report).toContain("agent_dx-image-input");
    expect(report).toContain("FAIL");
    expect(report).toContain("[tool:grep]");
    expect(report).toContain("verified=fail");
    expect(report).toContain("VERIFY FAIL: dead model");
  });

  it("handles rows without messages", () => {
    const [trace] = tracesFromResultRows([sampleRow({})]);
    expect(trace?.events).toEqual([]);
    expect(agentEventStreamFromMessages("not json")).toBe("");
  });
});
