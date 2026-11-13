import { describe, expect, it } from "bun:test";

import * as reference from "change-case";
import {
  assert,
  constantFrom,
  oneof,
  property,
  string,
  stringMatching,
} from "fast-check";

import { camelCase, snakeCase } from "./casing";

const RUNS = 20_000;

const TRICKY_CHARACTERS = [
  "a",
  "b",
  "z",
  "A",
  "B",
  "Z",
  "0",
  "1",
  "9",
  "_",
  "-",
  " ",
  ".",
  "$",
  "\0",
  "\t",
  "é",
  "É",
  "ß",
  "ı",
  "İ",
  "ǅ",
  "ﬀ",
  "\u0345",
  "\u0307",
  "\u212A",
  "Σ",
  "ς",
  "𝐀",
  "𝐚",
  "😀",
  "\uD800",
  "\uDC00",
  "\u00A0",
  "\uFEFF",
  "٣",
  "ᾈ",
];

const sdkKey = stringMatching(
  /^[a-z][a-z0-9]{0,8}(?:[A-Z][a-z0-9]{0,8}){0,4}$/
);
const wireKey = stringMatching(/^[a-z][a-z0-9]{0,8}(?:_[a-z0-9]{1,8}){0,4}$/);
const asciiKey = string({
  unit: constantFrom(...TRICKY_CHARACTERS.slice(0, 16)),
  maxLength: 24,
});
const trickyKey = string({
  unit: constantFrom(...TRICKY_CHARACTERS),
  maxLength: 24,
});
const anyKey = oneof(
  sdkKey,
  wireKey,
  asciiKey,
  trickyKey,
  string({ unit: "binary", maxLength: 24 }),
  string({ unit: "grapheme", maxLength: 12 })
);

type Outcome =
  | { kind: "value"; value: string }
  | { kind: "error"; name: string; message: string };

function outcome(convert: (input: string) => string, input: string): Outcome {
  try {
    return { kind: "value", value: convert(input) };
  } catch (error) {
    return error instanceof Error
      ? { kind: "error", name: error.name, message: error.message }
      : { kind: "error", name: typeof error, message: String(error) };
  }
}

const CONVERTERS = [
  ["snakeCase", snakeCase, reference.snakeCase],
  ["camelCase", camelCase, reference.camelCase],
] as const;

describe("case conversion conformance", () => {
  for (const [name, own, original] of CONVERTERS) {
    it(`${name} matches change-case for every caller key shape`, () => {
      assert(
        property(anyKey, (key) => {
          expect(outcome(own, key)).toEqual(
            outcome((input) => original(input), key)
          );
        }),
        { numRuns: RUNS }
      );
    });
  }

  it.each([
    ["previousResponseId", "previous_response_id", "previousResponseId"],
    ["previous_response_id", "previous_response_id", "previousResponseId"],
    ["XMLHttpRequest", "xml_http_request", "xmlHttpRequest"],
    ["max_tokens_2", "max_tokens_2", "maxTokens_2"],
    ["__type__", "type", "type"],
    ["fooBAR école", "foo_bar_école", "fooBarÉcole"],
    ["", "", ""],
  ])("converts %p", (input, snake, camel) => {
    expect([snakeCase(input), camelCase(input)]).toEqual([snake, camel]);
  });
});
