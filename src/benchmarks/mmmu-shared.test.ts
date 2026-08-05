import { describe, expect, it } from "bun:test";

import { parseOptions } from "./mmmu-shared";
describe("parseOptions", () => {
  it("parses Python-style single-quoted list", () => {
    const result = parseOptions("['$6', '$7', '$8', '$9']");
    expect(result).toEqual(["$6", "$7", "$8", "$9"]);
  });
  it("parses JSON double-quoted array", () => {
    const result = parseOptions('["A", "B", "C"]');
    expect(result).toEqual(["A", "B", "C"]);
  });
  it("returns empty for non-array input", () => {
    expect(parseOptions("not an array")).toEqual([]);
  });
  it("handles already-parsed array", () => {
    expect(parseOptions(["x", "y"])).toEqual(["x", "y"]);
  });
  it("returns empty for empty brackets", () => {
    expect(parseOptions("[]")).toEqual([]);
  });
});
