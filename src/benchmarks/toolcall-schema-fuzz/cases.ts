import type { ToolCallSchemaFuzzGenerator } from "./options";
import { TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES } from "./realistic/wordings";
import { TOOLCALL_SCHEMA_FUZZ_REGRESSION_CASES } from "./regressions";
import { sentinelCases } from "./sentinel";
import type { ToolCallSchemaFuzzCase } from "./tool-case";

export interface CaseSelection {
  readonly generator: ToolCallSchemaFuzzGenerator;
}

const GENERATORS: Readonly<
  Record<ToolCallSchemaFuzzGenerator, () => readonly ToolCallSchemaFuzzCase[]>
> = {
  realistic: () => TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES,
  regressions: () => TOOLCALL_SCHEMA_FUZZ_REGRESSION_CASES,
  sentinel: () =>
    sentinelCases({
      grid: TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES,
      regressions: TOOLCALL_SCHEMA_FUZZ_REGRESSION_CASES,
    }),
};

export function toolCallSchemaFuzzCases({
  generator,
}: CaseSelection): readonly ToolCallSchemaFuzzCase[] {
  return GENERATORS[generator]();
}

export type CaseLookup = ReadonlyMap<string, ToolCallSchemaFuzzCase>;

export function caseLookup(
  cases: readonly ToolCallSchemaFuzzCase[]
): CaseLookup {
  return new Map(cases.map((entry) => [entry.id, entry]));
}
