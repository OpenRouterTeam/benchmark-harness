import type { ModelUsage, ServerToolUseCounts } from "../../../harness/core";

function sum(values: readonly (number | undefined)[]): number | undefined {
  const defined = values.filter(
    (value): value is number => value !== undefined
  );
  return defined.length === 0
    ? undefined
    : defined.reduce((total, value) => total + value, 0);
}

function mergeServerToolUse(
  usages: readonly ServerToolUseCounts[]
): ServerToolUseCounts | undefined {
  if (usages.length === 0) {
    return undefined;
  }
  const webSearchRequests = sum(usages.map((usage) => usage.webSearchRequests));
  const toolCallsRequested = sum(
    usages.map((usage) => usage.toolCallsRequested)
  );
  const toolCallsExecuted = sum(usages.map((usage) => usage.toolCallsExecuted));
  return {
    ...(webSearchRequests !== undefined && { webSearchRequests }),
    ...(toolCallsRequested !== undefined && { toolCallsRequested }),
    ...(toolCallsExecuted !== undefined && { toolCallsExecuted }),
  };
}

export function mergeModelUsages(
  usages: readonly (ModelUsage | undefined)[]
): ModelUsage | undefined {
  const defined = usages.filter(
    (usage): usage is ModelUsage => usage !== undefined
  );
  if (defined.length === 0) {
    return undefined;
  }
  if (defined.length === 1) {
    return defined[0];
  }
  const inputTokens = sum(defined.map((usage) => usage.inputTokens));
  const outputTokens = sum(defined.map((usage) => usage.outputTokens));
  const totalTokens = sum(defined.map((usage) => usage.totalTokens));
  const reasoningTokens = sum(defined.map((usage) => usage.reasoningTokens));
  const totalCost = sum(defined.map((usage) => usage.totalCost));
  const serverToolUse = mergeServerToolUse(
    defined
      .map((usage) => usage.serverToolUse)
      .filter((usage): usage is ServerToolUseCounts => usage !== undefined)
  );
  return {
    ...(inputTokens !== undefined && { inputTokens }),
    ...(outputTokens !== undefined && { outputTokens }),
    ...(totalTokens !== undefined && { totalTokens }),
    ...(reasoningTokens !== undefined && { reasoningTokens }),
    ...(totalCost !== undefined && { totalCost }),
    ...(serverToolUse !== undefined && { serverToolUse }),
  };
}
