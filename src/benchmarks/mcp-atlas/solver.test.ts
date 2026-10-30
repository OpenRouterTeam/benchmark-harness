import { describe, expect, it } from "bun:test";

import { selectTools } from "./solver";

const tools = [
  { name: "git_status", description: "status" },
  { name: "weather_forecast" },
  { name: "wikipedia_search" },
] as const;

describe("selectTools", () => {
  it("sends only the task's enabled tools in enabled scope", () => {
    const selected = selectTools(
      tools,
      ["git_status", "missing_tool"],
      "enabled"
    );
    expect(selected.definitions.map((tool) => tool.function.name)).toEqual([
      "git_status",
    ]);
    expect(selected.missingTools).toEqual(["missing_tool"]);
  });

  it("sends every sandbox tool in all scope", () => {
    const selected = selectTools(tools, ["git_status"], "all");
    expect(selected.definitions).toHaveLength(3);
    expect(selected.missingTools).toEqual([]);
  });
});
