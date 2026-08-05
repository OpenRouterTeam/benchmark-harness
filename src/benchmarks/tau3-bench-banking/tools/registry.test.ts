import { describe, it, expect } from "bun:test";

import { makeEmptyBankingData } from "../environment";
import type { BankingData } from "../types";
import type { DiscoverableTool } from "./registry";
import {
  makeBankingEnvState,
  DISCOVERABLE_AGENT_TOOLS,
  DISCOVERABLE_USER_TOOLS,
  registerDiscoverableAgentTool,
  registerDiscoverableUserTool,
  formatDiscoverableToolForAgent,
} from "./registry";

function createTestDb(): BankingData {
  return makeEmptyBankingData();
}
describe("registry", () => {
  describe("makeBankingEnvState", () => {
    it("creates new environment state with empty registries", () => {
      const db = createTestDb();
      const state = makeBankingEnvState(db);
      expect(state.db).toBe(db);
      expect(state.unlockedAgentTools).toBeInstanceOf(Set);
      expect(state.givenUserTools).toBeInstanceOf(Map);
      expect(state.unlockedAgentTools.size).toBe(0);
      expect(state.givenUserTools.size).toBe(0);
    });
  });
  describe("registerDiscoverableAgentTool", () => {
    it("adds tool to DISCOVERABLE_AGENT_TOOLS registry", () => {
      const toolName = `test_agent_tool_${Date.now()}`;
      const tool: DiscoverableTool = {
        name: toolName,
        description: "Test tool",
        params: [],
        handler: () => "test result",
      };
      registerDiscoverableAgentTool(tool);
      expect(DISCOVERABLE_AGENT_TOOLS.has(toolName)).toBe(true);
    });
    it("stored tool can be retrieved", () => {
      const toolName = `test_agent_tool_${Date.now()}`;
      const tool: DiscoverableTool = {
        name: toolName,
        description: "Test tool",
        params: [
          {
            name: "param1",
            type: "string",
            optional: false,
            description: "First param",
          },
        ],
        handler: () => "result",
      };
      registerDiscoverableAgentTool(tool);
      const retrieved = DISCOVERABLE_AGENT_TOOLS.get(toolName);
      expect(retrieved).toBe(tool);
    });
  });
  describe("registerDiscoverableUserTool", () => {
    it("adds tool to DISCOVERABLE_USER_TOOLS registry", () => {
      const toolName = `test_user_tool_${Date.now()}`;
      const tool: DiscoverableTool = {
        name: toolName,
        description: "User tool",
        params: [],
        handler: () => "user result",
      };
      registerDiscoverableUserTool(tool);
      expect(DISCOVERABLE_USER_TOOLS.has(toolName)).toBe(true);
    });
  });
  describe("formatDiscoverableToolForAgent", () => {
    it("formats tool with no parameters", () => {
      const tool: DiscoverableTool = {
        name: "test_tool",
        description: "A test tool",
        params: [],
        handler: () => "",
      };
      const formatted = formatDiscoverableToolForAgent(tool);
      expect(formatted).toContain("Tool: test_tool");
      expect(formatted).toContain("Description: A test tool");
      expect(formatted).toContain("Parameters:");
      expect(formatted).toContain("(no parameters)");
    });
    it("formats tool with required parameters", () => {
      const tool: DiscoverableTool = {
        name: "test_tool",
        description: "A test tool",
        params: [
          {
            name: "user_id",
            type: "string",
            optional: false,
            description: "The user ID",
          },
          {
            name: "amount",
            type: "number",
            optional: false,
            description: "Transfer amount",
          },
        ],
        handler: () => "",
      };
      const formatted = formatDiscoverableToolForAgent(tool);
      expect(formatted).toContain("user_id: string (required) - The user ID");
      expect(formatted).toContain(
        "amount: number (required) - Transfer amount"
      );
    });
    it("formats tool with optional parameters", () => {
      const tool: DiscoverableTool = {
        name: "test_tool",
        description: "A test tool",
        params: [
          {
            name: "page",
            type: "integer",
            optional: true,
            description: "Page number",
          },
        ],
        handler: () => "",
      };
      const formatted = formatDiscoverableToolForAgent(tool);
      expect(formatted).toContain("page: integer (optional) - Page number");
    });
    it("includes tool name and description", () => {
      const tool: DiscoverableTool = {
        name: "transfer_funds",
        description: "Transfer money between accounts",
        params: [],
        handler: () => "",
      };
      const formatted = formatDiscoverableToolForAgent(tool);
      expect(formatted).toContain("Tool: transfer_funds");
      expect(formatted).toContain(
        "Description: Transfer money between accounts"
      );
    });
  });
  describe("State isolation", () => {
    it("multiple env states have independent tool registries", () => {
      const db1 = createTestDb();
      const db2 = createTestDb();
      const state1 = makeBankingEnvState(db1);
      const state2 = makeBankingEnvState(db2);
      state1.unlockedAgentTools.add("tool_a");
      expect(state2.unlockedAgentTools.has("tool_a")).toBe(false);
    });
    it("multiple env states have independent given user tools", () => {
      const db1 = createTestDb();
      const db2 = createTestDb();
      const state1 = makeBankingEnvState(db1);
      const state2 = makeBankingEnvState(db2);
      state1.givenUserTools.set("tool_x", { arg: "value" });
      expect(state2.givenUserTools.has("tool_x")).toBe(false);
    });
  });
});
