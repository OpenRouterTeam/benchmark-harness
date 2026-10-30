import type { ValueOf } from "../../internal/guards";

export const LIVE_MCP_BENCH_ID = "live_mcp_bench";

export const LIVE_MCP_BENCH_TOOL_SCOPES = ["all", "annotated"] as const;

export type LiveMcpBenchToolScope = ValueOf<typeof LIVE_MCP_BENCH_TOOL_SCOPES>;

export const LIVE_MCP_BENCH_CATEGORIES = [
  "Office",
  "Lifestyle",
  "Leisure",
  "Finance",
  "Travel",
  "Shopping",
] as const;

export type LiveMcpBenchCategory = ValueOf<typeof LIVE_MCP_BENCH_CATEGORIES>;

export const LIVE_MCP_BENCH_DEFAULT_SANDBOX_URL = "http://127.0.0.1:1985";

export const LIVE_MCP_BENCH_SANDBOX_TOKEN_ENV = "LIVE_MCP_BENCH_SANDBOX_TOKEN";

export const LIVE_MCP_BENCH_DEFAULT_MAX_TURNS = 30;

export const LIVE_MCP_BENCH_DEFAULT_MAX_TOOL_CALLS = 60;

export const LIVE_MCP_BENCH_DEFAULT_TOOL_OUTPUT_CAP = 40_000;

export const LIVE_MCP_BENCH_DEFAULT_JUDGE_MODEL = "anthropic/claude-sonnet-4.5";

export const LIVE_MCP_BENCH_SYSTEM_PROMPT =
  "You are an agent designed to assist users with daily tasks by using external tools. Use the available tools to complete the task.\n\nNote that you can only response to user once, so you should try to provide a complete answer in your response.\n";

export const LIVE_MCP_BENCH_JUDGE_KINDS = ["model", "jev"] as const;

export type LiveMcpBenchJudgeKind = ValueOf<typeof LIVE_MCP_BENCH_JUDGE_KINDS>;

export const LIVE_MCP_BENCH_DEFAULT_JEV_MODEL = "jev-latest";

export const LIVE_MCP_BENCH_JUDGE_OUTPUT_CAP = 4_000;
