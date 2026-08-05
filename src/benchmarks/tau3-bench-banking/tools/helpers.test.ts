import { describe, expect, it } from "bun:test";

import {
  FROZEN_TODAY_STR,
  getAccountBalance,
  getTodayStr,
  parseBalance,
  parseToolArguments,
} from "./helpers";
describe("getTodayStr", () => {
  it("returns the frozen clock date", () => {
    expect(getTodayStr()).toBe("11/14/2025");
    expect(getTodayStr()).toBe(FROZEN_TODAY_STR);
  });
});
describe("parseBalance", () => {
  it("passes numbers through", () => {
    expect(parseBalance(42.5)).toBe(42.5);
  });
  it("strips $ and commas from strings", () => {
    expect(parseBalance("$1,234.56")).toBe(1234.56);
  });
  it("returns 0 for non-numeric values", () => {
    expect(parseBalance(null)).toBe(0);
    expect(parseBalance({})).toBe(0);
    expect(parseBalance(undefined)).toBe(0);
    expect(parseBalance(Number.NaN)).toBe(0);
    expect(parseBalance(Number.POSITIVE_INFINITY)).toBe(0);
  });
});
describe("getAccountBalance", () => {
  it("uses current holdings before balance", () => {
    expect(
      getAccountBalance({ current_holdings: "$2,850.00", balance: "$1.00" })
    ).toBe(2850);
  });
  it("falls back to balance for null or missing current holdings", () => {
    expect(
      getAccountBalance({ current_holdings: null, balance: "$1.00" })
    ).toBe(1);
    expect(getAccountBalance({ balance: "$1.00" })).toBe(1);
  });
});
describe("parseToolArguments", () => {
  it("parses a JSON object", () => {
    expect(parseToolArguments('{"user_id":"u1"}')).toEqual({ user_id: "u1" });
  });
  it("coerces malformed JSON to an empty object", () => {
    expect(parseToolArguments("not json")).toEqual({});
  });
  it("coerces non-object JSON to an empty object", () => {
    expect(parseToolArguments("[1,2]")).toEqual({});
    expect(parseToolArguments('"str"')).toEqual({});
  });
});
