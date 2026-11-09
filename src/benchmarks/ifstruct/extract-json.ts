import * as Result from "effect/Result";

import { unknownErrorToString } from "../../internal/errors";
import { extractOuterFencedBlock, matchJsonContainer } from "./extract-common";
import { pyRepr } from "./py-format";

export type ExtractResult = Result.Result<unknown, string>;

export function extractJson(response: string): ExtractResult {
  const trimmed = response.trim();
  const { block, openingFence } = extractOuterFencedBlock(trimmed);
  if (openingFence !== null && block === null) {
    return Result.fail("Unclosed code block");
  }
  if (block !== null) {
    const lowered = (openingFence ?? "").trim().toLowerCase();
    if (lowered.startsWith("```yaml") || lowered.startsWith("```yml")) {
      return Result.fail("Expected JSON output, got YAML code block");
    }
    return loadCompleteJson(block.trim());
  }
  if (trimmed === "" || (trimmed[0] !== "[" && trimmed[0] !== "{")) {
    return Result.fail("No valid JSON found in response");
  }
  return loadCompleteJson(trimmed);
}

export function loadCompleteJson(content: string): ExtractResult {
  const trimmed = content.trim();
  if (trimmed === "") {
    return Result.fail("No valid JSON found in response");
  }
  const end = scanJsonValue(trimmed, 0);
  if (end === null) {
    return Result.fail("JSON parse error: no JSON value found");
  }
  const head = trimmed.slice(0, end);
  const parsed = Result.try((): unknown => JSON.parse(head));
  if (Result.isFailure(parsed)) {
    return Result.fail(
      `JSON parse error: ${unknownErrorToString(parsed.failure)}`
    );
  }
  const trailing = trimmed.slice(end).trim();
  if (trailing !== "") {
    const preview =
      trailing.length > 100 ? `${trailing.slice(0, 100)}...` : trailing;
    return Result.fail(`Trailing content after JSON: ${pyRepr(preview)}`);
  }
  return Result.succeed(parsed.success);
}

function scanJsonValue(s: string, start: number): number | null {
  const c = s[start];
  if (c === "{" || c === "[") {
    return matchJsonContainer({
      text: s,
      start,
      open: c,
      close: c === "{" ? "}" : "]",
    });
  }
  if (c === '"') {
    return scanString(s, start);
  }
  const literal = /^(?:true|false|null|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)/.exec(
    s.slice(start)
  );
  return literal !== null ? start + literal[0].length : null;
}

function scanString(s: string, start: number): number | null {
  let escapeNext = false;
  for (let i = start + 1; i < s.length; i++) {
    const ch = s[i];
    if (escapeNext) {
      escapeNext = false;
      continue;
    }
    if (ch === "\\") {
      escapeNext = true;
      continue;
    }
    if (ch === '"') {
      return i + 1;
    }
  }
  return null;
}
