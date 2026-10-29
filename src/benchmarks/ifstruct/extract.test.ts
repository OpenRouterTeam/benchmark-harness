import { describe, expect, it } from "bun:test";

import { assertSuccess, assertFailure } from "../../internal/testing";
import {
  checkForCommentary,
  checkForCommentaryYaml,
  checkUsesCodeBlock,
  removeThinkingTags,
} from "./extract-common";
import { extractJson } from "./extract-json";
import { extractYaml, JSON_AS_YAML_ERROR } from "./extract-yaml";
describe("removeThinkingTags", () => {
  it("strips a paired <think> block", () => {
    expect(removeThinkingTags('<think>draft junk</think>[{"id": "1"}]')).toBe(
      '[{"id": "1"}]'
    );
  });
  it("strips a dangling closing </think> and everything before it", () => {
    expect(removeThinkingTags("leading reasoning</think>final")).toBe("final");
  });
  it("strips a dangling opening <think> and everything after it", () => {
    expect(removeThinkingTags("keep<think>trailing reasoning")).toBe("keep");
  });
  it("keeps only the harmony final channel payload", () => {
    const text =
      'analysis notes<|start|>assistant<|channel|>final<|message|>[{"id":"1"}]<|end|>';
    expect(removeThinkingTags(text)).toBe('[{"id":"1"}]');
  });
});
describe("checkUsesCodeBlock", () => {
  it("reports the specific fence language when present", () => {
    expect(checkUsesCodeBlock("```json\n{}\n```")).toEqual({
      usesCodeBlock: true,
      codeBlockType: "```json",
    });
    expect(checkUsesCodeBlock("```yaml\nk: v\n```")).toEqual({
      usesCodeBlock: true,
      codeBlockType: "```yaml",
    });
  });
  it("falls back to a bare fence when no language is given", () => {
    expect(checkUsesCodeBlock("```\n{}\n```")).toEqual({
      usesCodeBlock: true,
      codeBlockType: "```",
    });
  });
  it("does not treat an unclosed fence as a code block", () => {
    expect(checkUsesCodeBlock('```json\n[{"id": "1"}]')).toEqual({
      usesCodeBlock: false,
      codeBlockType: null,
    });
  });
});
describe("extractJson", () => {
  it("parses raw JSON without a fence", () => {
    const result = extractJson('[{"id": "1"}]');
    assertSuccess(result);
    expect(result.success).toEqual([{ id: "1" }]);
  });
  it("parses JSON inside a fence", () => {
    const result = extractJson('```json\n{"id": "1"}\n```');
    assertSuccess(result);
    expect(result.success).toEqual({ id: "1" });
  });
  it("rejects an unclosed code block", () => {
    const result = extractJson('```json\n[{"id": "1"}]');
    assertFailure(result);
    expect(result.failure).toBe("Unclosed code block");
  });
  it("rejects a trailing extra brace", () => {
    const result = extractJson('{"id": "1"}\n}');
    assertFailure(result);
    expect(result.failure).toContain("Trailing content after JSON");
  });
  it("rejects trailing prose after the value", () => {
    const result = extractJson('[{"id": "1"}]\nok');
    assertFailure(result);
    expect(result.failure).toContain("Trailing content after JSON");
  });
  it("rejects a trailing extra brace inside a fence", () => {
    const result = extractJson('```json\n{"id": "1"}\n}\n```');
    assertFailure(result);
    expect(result.failure).toContain("Trailing content after JSON");
  });
  it("rejects a YAML fence when JSON was requested", () => {
    const result = extractJson('```yaml\n{"id": "1"}\n```');
    assertFailure(result);
    expect(result.failure).toBe("Expected JSON output, got YAML code block");
  });
  it("rejects a response that does not start with a JSON delimiter", () => {
    const result = extractJson("here you go");
    assertFailure(result);
    expect(result.failure).toBe("No valid JSON found in response");
  });
});
describe("extractYaml", () => {
  it("parses raw block YAML without a fence", () => {
    const result = extractYaml("- id: '1'");
    assertSuccess(result);
    expect(result.success).toEqual([{ id: "1" }]);
  });
  it("rejects an unclosed code block", () => {
    const result = extractYaml("```yaml\n- id: '1'");
    assertFailure(result);
    expect(result.failure).toBe("Unclosed code block");
  });
  it("rejects a JSON fence when YAML was requested", () => {
    const result = extractYaml("```json\n- id: '1'\n```");
    assertFailure(result);
    expect(result.failure).toBe("Expected YAML output, got JSON code block");
  });
  it("rejects flow-style (JSON-shaped) content inside a yaml fence", () => {
    const result = extractYaml('```yaml\n[{"id": "1"}]\n```');
    assertFailure(result);
    expect(result.failure).toBe(JSON_AS_YAML_ERROR);
  });
  it("accepts idiomatic block YAML with a nested flow scalar sequence", () => {
    const result = extractYaml("```yaml\ntags: [a, b]\n```");
    assertSuccess(result);
    expect(result.success).toEqual({ tags: ["a", "b"] });
  });
  it("accepts duplicate mapping keys with last-wins semantics, like PyYAML", () => {
    const result = extractYaml("```yaml\na: x\nb: y\na: z\n```");
    assertSuccess(result);
    expect(result.success).toEqual({ a: "z", b: "y" });
  });
});
describe("checkForCommentary", () => {
  it("flags prose after a JSON code block", () => {
    const { hasCommentary, description } = checkForCommentary(
      '```json\n[{"id": "1"}]\n```\nok'
    );
    expect(hasCommentary).toBe(true);
    expect(description).toBe('Response contains text outside JSON: "ok"');
  });
  it("does not flag a clean fenced response", () => {
    expect(checkForCommentary('```json\n[{"id": "1"}]\n```')).toEqual({
      hasCommentary: false,
      description: null,
    });
  });
});
describe("checkForCommentaryYaml", () => {
  it("flags prose after a YAML code block", () => {
    const { hasCommentary, description } = checkForCommentaryYaml(
      "```yaml\n- id: '1'\n```\nok"
    );
    expect(hasCommentary).toBe(true);
    expect(description).toBe('Response contains text outside YAML: "ok"');
  });
  it("does not flag unfenced YAML (commentary only checked inside fences)", () => {
    expect(checkForCommentaryYaml("- id: '1'")).toEqual({
      hasCommentary: false,
      description: null,
    });
  });
});
