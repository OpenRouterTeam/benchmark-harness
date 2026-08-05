import { describe, expect, it } from "bun:test";

import { parseSingleVerdict } from "./parse-verdict";
describe("parseSingleVerdict", () => {
  it("parses a direct JSON object", () => {
    expect(
      parseSingleVerdict('{"verdict":"MET","justification":"cited X"}')
    ).toEqual(["MET", "cited X"]);
  });
  it("parses UNMET", () => {
    expect(
      parseSingleVerdict('{"verdict":"UNMET","justification":"missing"}')
    ).toEqual(["UNMET", "missing"]);
  });
  it("coerces a non-string justification to a string", () => {
    expect(
      parseSingleVerdict('{"verdict":"MET","justification":["a","b"]}')
    ).toEqual(["MET", "a,b"]);
  });
  it("tolerates a missing justification (defaults to empty string)", () => {
    expect(parseSingleVerdict('{"verdict":"MET"}')).toEqual(["MET", ""]);
  });
  it("returns null for an invalid verdict value", () => {
    expect(
      parseSingleVerdict('{"verdict":"MAYBE","justification":"x"}')
    ).toBeNull();
  });
  it("returns null for unparsable / non-JSON content", () => {
    expect(parseSingleVerdict("the response was good")).toBeNull();
    expect(parseSingleVerdict("")).toBeNull();
  });
  it("does not salvage a fenced / embedded / trailing-brace verdict", () => {
    expect(
      parseSingleVerdict(
        '```json\n{"verdict":"MET","justification":"yes"}\n```'
      )
    ).toBeNull();
    expect(
      parseSingleVerdict('prose {"verdict":"UNMET","justification":"no"} more')
    ).toBeNull();
    expect(
      parseSingleVerdict(
        '{"verdict":"MET","justification":"x"} Rubric: {"section":"F"}'
      )
    ).toBeNull();
    expect(
      parseSingleVerdict('{"verdict":"MET","justification":"x"}\n}')
    ).toBeNull();
  });
  it("parses a JSON object whose justification string contains braces", () => {
    expect(
      parseSingleVerdict('{"verdict":"MET","justification":"see {example}"}')
    ).toEqual(["MET", "see {example}"]);
  });
});
