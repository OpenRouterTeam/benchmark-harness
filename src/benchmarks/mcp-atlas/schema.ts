import type { ValueOf } from "../../internal/guards";

export const MCP_ATLAS_ID = "mcp_atlas";

export const MCP_ATLAS_TOOL_SCOPES = ["enabled", "all"] as const;

export type McpAtlasToolScope = ValueOf<typeof MCP_ATLAS_TOOL_SCOPES>;

export const MCP_ATLAS_DEFAULT_SANDBOX_URL = "http://127.0.0.1:1984";

export const MCP_ATLAS_SANDBOX_TOKEN_ENV = "MCP_ATLAS_SANDBOX_TOKEN";

export const MCP_ATLAS_DEFAULT_JUDGE_MODEL = "google/gemini-3.1-pro-preview";

export const MCP_ATLAS_DEFAULT_PASS_THRESHOLD = 0.75;

export const MCP_ATLAS_DEFAULT_MAX_TURNS = 256;

export const MCP_ATLAS_DEFAULT_MAX_TOOL_CALLS = 100;

export const MCP_ATLAS_NO_KEY_SERVERS = [
  "arxiv",
  "calculator",
  "cli-mcp-server",
  "clinicaltrialsgov-mcp-server",
  "context7",
  "ddg-search",
  "desktop-commander",
  "fetch",
  "filesystem",
  "git",
  "mcp-code-executor",
  "mcp-server-code-runner",
  "memory",
  "met-museum",
  "open-library",
  "osm-mcp-server",
  "pubmed",
  "weather",
  "whois",
  "wikipedia",
] as const;
