import { Either } from "../../internal/either";
import { parseSchema, z } from "../../internal/zod";
import { constructsOf } from "./constructs";
import { CaseWordingSchema } from "./realistic/wordings";
import data from "./regressions.json" with { type: "json" };
import type { ToolCallSchemaFuzzCase } from "./tool-case";
import { buildCase, FuzzScenario } from "./tool-case";

const ArgsSchema = z.record(z.string(), z.unknown());

const RegressionToolSchema = z.object({
  name: z.string(),
  strict: z.boolean(),
  parameters: ArgsSchema,
  args: ArgsSchema,

  wording: CaseWordingSchema.optional(),
});

const ObservedSchema = z.object({ failures: z.number(), runs: z.number() });

const RegressionEntrySchema = z.object({
  key: z.string(),
  failing: RegressionToolSchema,
  control: RegressionToolSchema.optional(),
  finding: z.object({
    category: z.string(),
    explanation: z.string(),
    model: z.string(),
    provider: z.string(),
    observed: ObservedSchema,
    reference: z
      .object({ provider: z.string(), observed: ObservedSchema })
      .optional(),
    discoveredAt: z.string(),
  }),
});
export type RegressionEntry = z.infer<typeof RegressionEntrySchema>;

export const RegressionFileSchema = z.object({
  version: z.number(),
  entries: z.array(RegressionEntrySchema),
});
export type RegressionFile = z.infer<typeof RegressionFileSchema>;

const ID_PREFIX = "toolcall_schema_fuzz-regression:";

function regressionCase({
  tool,
  id,
  index,
  control,
}: {
  readonly tool: z.infer<typeof RegressionToolSchema>;
  readonly id: string;
  readonly index: number;
  readonly control?: string;
}): ToolCallSchemaFuzzCase {
  return buildCase({
    id,
    index,
    scenario: FuzzScenario.Single,
    name: tool.name,
    strict: tool.strict,
    parameters: tool.parameters,
    args: [tool.args, tool.args],
    constructs: constructsOf(tool.parameters),
    ...(tool.wording === undefined ? {} : { wording: tool.wording }),
    ...(control === undefined ? {} : { control }),
  });
}

export function regressionCases(
  file: RegressionFile
): readonly ToolCallSchemaFuzzCase[] {
  return file.entries
    .flatMap((entry) => {
      const id = `${ID_PREFIX}${entry.key}`;
      const controlId = `${id}:control`;
      return [
        {
          tool: entry.failing,
          id,
          ...(entry.control === undefined ? {} : { control: controlId }),
        },
        ...(entry.control === undefined
          ? []
          : [{ tool: entry.control, id: controlId }]),
      ];
    })
    .map((input, index) => regressionCase({ ...input, index }));
}

export const TOOLCALL_SCHEMA_FUZZ_REGRESSIONS: RegressionFile =
  Either.getOrThrow(parseSchema(RegressionFileSchema, data));

export const TOOLCALL_SCHEMA_FUZZ_REGRESSION_CASES: readonly ToolCallSchemaFuzzCase[] =
  regressionCases(TOOLCALL_SCHEMA_FUZZ_REGRESSIONS);
