import { isRecord } from "../../internal/guards";
import type { Tau3Action } from "./types";

function stableStringify(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(stableStringify).join(",")}]`;
  }
  if (isRecord(value)) {
    const entries = Object.keys(value)
      .toSorted()
      .map((k) => `${JSON.stringify(k)}:${stableStringify(value[k])}`);
    return `{${entries.join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}

function deepEqual(a: unknown, b: unknown): boolean {
  return stableStringify(a) === stableStringify(b);
}

export function compareActionWithToolCall(
  action: Tau3Action,
  toolName: string,
  toolArgs: Readonly<Record<string, unknown>>
): boolean {
  if (action.name !== toolName) {
    return false;
  }
  const compareArgs = action.compare_args ?? Object.keys(toolArgs);
  return compareArgs.every(
    (key) =>
      key in toolArgs === key in action.arguments &&
      deepEqual(toolArgs[key], action.arguments[key])
  );
}
