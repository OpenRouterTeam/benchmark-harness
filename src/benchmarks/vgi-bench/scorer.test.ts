import { describe, expect, it } from "bun:test";

import { parseVgiAnswer } from "./scorer";

describe("parseVgiAnswer", () => {
  it("parses a bare lowercase letter with a closing paren", () => {
    expect(parseVgiAnswer("d)", 4)).toBe("D");
  });

  it("parses a bare lowercase letter with a period", () => {
    expect(parseVgiAnswer("b.", 4)).toBe("B");
  });

  it("parses a bare single letter", () => {
    expect(parseVgiAnswer("c", 4)).toBe("C");
  });

  it("parses an uppercase bare letter", () => {
    expect(parseVgiAnswer("A", 4)).toBe("A");
  });

  it("parses an explicit 'Answer:' line (lowercase)", () => {
    expect(parseVgiAnswer("Answer: c", 4)).toBe("C");
  });

  it("parses an explicit 'Answer:' line with markdown emphasis", () => {
    expect(parseVgiAnswer("Answer: **d**", 4)).toBe("D");
  });

  it("prefers the explicit answer line over a standalone article 'a'", () => {
    expect(parseVgiAnswer("the answer is c", 4)).toBe("C");
  });

  it("falls back to the first standalone letter when there is no answer line", () => {
    expect(parseVgiAnswer("a", 4)).toBe("A");
  });

  it("bounds the standalone-letter fallback to the option count", () => {
    expect(parseVgiAnswer("z", 4)).toBeNull();
    expect(parseVgiAnswer("e", 4)).toBeNull();
    expect(parseVgiAnswer("d", 4)).toBe("D");
  });

  it("bounds the explicit answer line to the option count", () => {
    expect(parseVgiAnswer("Answer: z", 4)).toBeNull();
  });

  it("returns null for empty or unparseable text", () => {
    expect(parseVgiAnswer("", 4)).toBeNull();
    expect(parseVgiAnswer("I cannot tell.", 4)).toBeNull();
  });

  it("returns null when nOptions is zero (defensive)", () => {
    expect(parseVgiAnswer("a", 0)).toBeNull();
  });

  it("handles a letter followed by option text", () => {
    expect(parseVgiAnswer("d) kitchen scissors", 4)).toBe("D");
  });
});
