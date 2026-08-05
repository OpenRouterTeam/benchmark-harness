import type { BankingData } from "../types";

type BankingDb = BankingData;

export interface ToolParameter {
  readonly name: string;
  readonly type: string;
  readonly optional: boolean;
  readonly description: string;
}

export interface DiscoverableTool {
  readonly name: string;
  readonly description: string;
  readonly params: readonly ToolParameter[];
  readonly mutatesState?: boolean;
  readonly handler: (
    state: BankingEnvState,
    kwargs: Record<string, unknown>
  ) => string;
}

export interface BankingEnvState {
  db: BankingDb;
  unlockedAgentTools: Set<string>;
  givenUserTools: Map<string, Record<string, unknown>>;
  readLogAllowlist: ReadonlySet<string>;
}

export function makeBankingEnvState(
  db: BankingDb,
  readLogAllowlist: Iterable<string> = []
): BankingEnvState {
  return {
    db,
    unlockedAgentTools: new Set(),
    givenUserTools: new Map(),
    readLogAllowlist: new Set(readLogAllowlist),
  };
}

export const DISCOVERABLE_AGENT_TOOLS = new Map<string, DiscoverableTool>();

export const DISCOVERABLE_USER_TOOLS = new Map<string, DiscoverableTool>();

export function registerDiscoverableAgentTool(tool: DiscoverableTool): void {
  DISCOVERABLE_AGENT_TOOLS.set(tool.name, tool);
}

export function registerDiscoverableUserTool(tool: DiscoverableTool): void {
  DISCOVERABLE_USER_TOOLS.set(tool.name, tool);
}

export function formatDiscoverableToolForAgent(tool: DiscoverableTool): string {
  const paramStrs: string[] = [];
  for (const param of tool.params) {
    const req = param.optional ? " (optional)" : " (required)";
    paramStrs.push(
      `  - ${param.name}: ${param.type}${req} - ${param.description}`
    );
  }
  const paramsSection =
    paramStrs.length > 0 ? paramStrs.join("\n") : "  (no parameters)";
  return `Tool: ${tool.name}\nDescription: ${tool.description}\nParameters:\n${paramsSection}`;
}
