import type { ToolCallSchemaFuzzCase } from "./tool-case";

export function sentinelCases({
  grid,
  regressions,
}: {
  readonly grid: readonly ToolCallSchemaFuzzCase[];
  readonly regressions: readonly ToolCallSchemaFuzzCase[];
}): readonly ToolCallSchemaFuzzCase[] {
  const firsts = new Map<string, ToolCallSchemaFuzzCase>();
  for (const entry of grid) {
    for (const construct of entry.constructs) {
      const key = `${entry.scenario}:${entry.strict}:${construct}`;
      if (!firsts.has(key)) {
        firsts.set(key, entry);
      }
    }
  }
  const picked = new Set([...firsts.values()].map((entry) => entry.id));
  const withControls = new Set([
    ...picked,
    ...[...firsts.values()].flatMap((entry) =>
      entry.control === undefined ? [] : [entry.control]
    ),
  ]);
  return [
    ...grid.filter((entry) => withControls.has(entry.id)),
    ...regressions,
  ].map((entry, index) => ({ ...entry, index }));
}
