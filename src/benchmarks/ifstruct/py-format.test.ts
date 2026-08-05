import { describe, expect, it } from "bun:test";

import { pyRepr, pyTypeName } from "./py-format";
describe("pyRepr", () => {
  it("single-quotes plain strings", () => {
    expect(pyRepr("abc")).toBe("'abc'");
  });
  it("double-quotes strings containing ' but no double quote, like CPython", () => {
    expect(pyRepr("it's")).toBe('"it\'s"');
  });
  it("single-quotes and escapes ' when both quote kinds are present", () => {
    expect(pyRepr('it\'s "x"')).toBe("'it\\'s \"x\"'");
  });
  it("escapes backslashes", () => {
    expect(pyRepr("a\\b")).toBe("'a\\\\b'");
  });
  it("formats scalars and lists Python-style", () => {
    expect(pyRepr(null)).toBe("None");
    expect(pyRepr(true)).toBe("True");
    expect(pyRepr([1, "a"])).toBe("[1, 'a']");
  });
});
describe("pyTypeName", () => {
  it("maps JSON values to Python type names", () => {
    expect(pyTypeName(null)).toBe("NoneType");
    expect(pyTypeName("x")).toBe("str");
    expect(pyTypeName(1)).toBe("int");
    expect(pyTypeName(1.5)).toBe("float");
    expect(pyTypeName([])).toBe("list");
    expect(pyTypeName({})).toBe("dict");
  });
});
