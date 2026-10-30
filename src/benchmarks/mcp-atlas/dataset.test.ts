import { describe, expect, it } from "bun:test";

import {
  parsePythonStringList,
  parseStringList,
  toolServerName,
  usesOnlyServers,
} from "./dataset";

describe("parsePythonStringList", () => {
  it("reads single-quoted Python lists", () => {
    expect(parsePythonStringList("['a', \"b\", 'c d']", "F")).toEqual([
      "a",
      "b",
      "c d",
    ]);
  });

  it("rejects text that is not a list", () => {
    expect(() => parsePythonStringList("nope", "F")).toThrow("F");
  });
});

describe("parseStringList", () => {
  it("accepts JSON arrays and Python lists", () => {
    expect(parseStringList('["x", "y"]', "F")).toEqual(["x", "y"]);
    expect(parseStringList("['x']", "F")).toEqual(["x"]);
    expect(parseStringList(["x"], "F")).toEqual(["x"]);
  });

  it("rejects non-string entries", () => {
    expect(() => parseStringList("[1]", "F")).toThrow("F");
  });
});

describe("toolServerName", () => {
  const servers = ["git", "mcp-server-code-runner", "weather"];

  it("maps a tool to the longest matching server prefix", () => {
    expect(toolServerName("mcp-server-code-runner_run_code", servers)).toBe(
      "mcp-server-code-runner"
    );
    expect(toolServerName("git_status", servers)).toBe("git");
  });

  it("returns undefined for tools of other servers", () => {
    expect(toolServerName("github_create_issue", servers)).toBeUndefined();
    expect(toolServerName("weatherman_lookup", servers)).toBeUndefined();
  });
});

describe("usesOnlyServers", () => {
  it("keeps tasks whose tools all belong to the allowed servers", () => {
    expect(
      usesOnlyServers(["git_status", "weather_forecast"], ["git", "weather"])
    ).toBe(true);
    expect(
      usesOnlyServers(["git_status", "github_search"], ["git", "weather"])
    ).toBe(false);
  });
});
