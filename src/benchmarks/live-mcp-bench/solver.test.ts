import { describe, expect, it } from "bun:test";

import { MessageRole } from "../../harness/core";
import { LIVE_MCP_BENCH_SYSTEM_PROMPT } from "./schema";
import {
  calledToolDescriptions,
  matchesAnnotatedTool,
  selectTools,
  withSystemPrompt,
} from "./solver";

const tools = [
  { name: "get-weread-rank", description: "WeRead ranking" },
  { name: "excel__read_sheet", description: "Read a sheet" },
  { name: "convert_to_pdf" },
] as const;

describe("matchesAnnotatedTool", () => {
  it("matches exact names and server-prefixed names", () => {
    expect(matchesAnnotatedTool("convert_to_pdf", "convert_to_pdf")).toBe(true);
    expect(matchesAnnotatedTool("excel__read_sheet", "read_sheet")).toBe(true);
  });

  it("does not match a different tool with a shared suffix", () => {
    expect(matchesAnnotatedTool("unread_sheet", "read_sheet")).toBe(false);
  });
});

describe("selectTools", () => {
  it("sends every tool in all scope", () => {
    expect(selectTools(tools, ["convert_to_pdf"], "all")).toHaveLength(3);
  });

  it("keeps only the annotated tools in annotated scope", () => {
    expect(
      selectTools(tools, ["read_sheet", "missing"], "annotated").map(
        (tool) => tool.name
      )
    ).toEqual(["excel__read_sheet"]);
  });
});

describe("withSystemPrompt", () => {
  it("prepends the upstream system prompt", () => {
    const messages = withSystemPrompt([
      { role: MessageRole.User, content: "task" },
    ]);
    expect(messages[0]).toEqual({
      role: MessageRole.System,
      content: LIVE_MCP_BENCH_SYSTEM_PROMPT,
    });
    expect(messages).toHaveLength(2);
  });

  it("leaves an existing system message alone", () => {
    const messages = [
      { role: MessageRole.System, content: "custom" },
      { role: MessageRole.User, content: "task" },
    ];
    expect(withSystemPrompt(messages)).toBe(messages);
  });
});

describe("calledToolDescriptions", () => {
  it("records each called tool once with its description", () => {
    expect(
      calledToolDescriptions(tools, [
        "get-weread-rank",
        "convert_to_pdf",
        "get-weread-rank",
        "unknown_tool",
      ])
    ).toEqual({
      "get-weread-rank": "WeRead ranking",
      convert_to_pdf: "(no description)",
      unknown_tool: "(no description)",
    });
  });
});
