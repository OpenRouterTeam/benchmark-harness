import * as Result from "effect/Result";
import type { Node } from "yaml";
import { isMap, isSeq, parse, parseDocument } from "yaml";

import { unknownErrorToString } from "../../internal/errors";
import { extractOuterFencedBlock } from "./extract-common";
import type { ExtractResult } from "./extract-json";

export const JSON_AS_YAML_ERROR =
  "Response is JSON/flow style, but YAML output was requested. " +
  "Emit block-style YAML (e.g. `key: value` / `- item`), not JSON wrapped " +
  "in a ```yaml fence.";

export function extractYaml(response: string): ExtractResult {
  const trimmed = response.trim();
  const { block, openingFence } = extractOuterFencedBlock(trimmed);
  if (openingFence !== null && block === null) {
    return Result.fail("Unclosed code block");
  }
  if (block !== null) {
    const lowered = (openingFence ?? "").trim().toLowerCase();
    if (lowered.startsWith("```json")) {
      return Result.fail("Expected YAML output, got JSON code block");
    }
    return loadBlockYaml(block.trim());
  }
  return loadBlockYaml(trimmed);
}

function loadBlockYaml(content: string): ExtractResult {
  const parsed = Result.try((): unknown =>
    parse(content, { uniqueKeys: false })
  );
  if (Result.isFailure(parsed)) {
    return Result.fail(
      `YAML parsing error: ${unknownErrorToString(parsed.failure)}`
    );
  }
  if (isJsonLikeYaml(content)) {
    return Result.fail(JSON_AS_YAML_ERROR);
  }
  return Result.succeed(parsed.success);
}

function isJsonLikeYaml(content: string): boolean {
  const doc = Result.try(() => parseDocument(content, { uniqueKeys: false }));
  if (Result.isFailure(doc)) {
    return false;
  }
  const root = doc.success.contents;
  if (root === null || root === undefined) {
    return false;
  }
  if ((isMap(root) || isSeq(root)) && root.flow === true) {
    return true;
  }
  return containsFlowMapping(root);
}

function containsFlowMapping(node: Node): boolean {
  if (isMap(node)) {
    if (node.flow === true) {
      return true;
    }
    return node.items.some(
      (pair) => isFlowNode(pair.key) || isFlowNode(pair.value)
    );
  }
  if (isSeq(node)) {
    return node.items.some((item) => isFlowNode(item));
  }
  return false;
}

function isFlowNode(value: unknown): boolean {
  return (isMap(value) || isSeq(value)) && containsFlowMapping(value);
}
