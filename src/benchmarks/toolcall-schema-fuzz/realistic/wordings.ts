import { Either } from "../../../internal/either";
import { parseSchema, z } from "../../../internal/zod";
import { constructsOf } from "../constructs";
import type { Slot } from "../grid";
import { GRID_SLOTS, gridShapeDraws, slotCaseId } from "../grid";
import type { ToolCallSchemaFuzzCase } from "../tool-case";
import { buildCase, FuzzScenario } from "../tool-case";
import data from "./wordings.json" with { type: "json" };

const ArgsSchema = z.record(z.string(), z.unknown());

export const CaseWordingSchema = z.object({
  description: z.string(),

  prompts: z.object({
    first: z.string(),
    second: z.string(),
    both: z.string().optional(),
  }),
  distractor: z.object({ name: z.string(), description: z.string() }),
});

export const WordingEntrySchema = CaseWordingSchema.extend({
  key: z.string(),
  name: z.string(),
  parameters: ArgsSchema,
  args: z.tuple([ArgsSchema, ArgsSchema]),
});
export type WordingEntry = z.infer<typeof WordingEntrySchema>;

export const WordingFileSchema = z.object({
  version: z.number(),
  writer: z.string(),
  checker: z.string(),
  entries: z.array(WordingEntrySchema),
});
export type WordingFile = z.infer<typeof WordingFileSchema>;

const ID_PREFIX = "toolcall_schema_fuzz-realistic:";

function isDistinct(entry: WordingEntry): boolean {
  return JSON.stringify(entry.args[0]) !== JSON.stringify(entry.args[1]);
}

function slotCase({
  slot,
  entry,
  index,
}: {
  readonly slot: Slot;
  readonly entry: WordingEntry;
  readonly index: number;
}): ToolCallSchemaFuzzCase {
  return buildCase({
    id: slotCaseId({ prefix: ID_PREFIX, slot, draw: 0 }),
    index,
    scenario: slot.scenario,
    strict: slot.strict,
    name: entry.name,
    parameters: entry.parameters,
    args: entry.args,
    constructs: constructsOf(entry.parameters),
    wording: {
      description: entry.description,
      prompts: entry.prompts,
      distractor: entry.distractor,
    },
  });
}

export function realisticCases(
  file: WordingFile
): readonly ToolCallSchemaFuzzCase[] {
  const byKey = new Map(file.entries.map((entry) => [entry.key, entry]));
  return GRID_SLOTS.flatMap((slot) => {
    const entry = byKey.get(slot.shape.key);
    if (
      entry === undefined ||
      (slot.scenario === FuzzScenario.Parallel &&
        (!isDistinct(entry) || entry.prompts.both === undefined))
    ) {
      return [];
    }
    return [{ slot, entry }];
  }).map(({ slot, entry }, index) => slotCase({ slot, entry, index }));
}

export function unwordedShapes(
  file: WordingFile
): ReturnType<typeof gridShapeDraws> {
  const done = new Set(file.entries.map((entry) => entry.key));
  return gridShapeDraws().filter(({ shape }) => !done.has(shape.key));
}

export const TOOLCALL_SCHEMA_FUZZ_WORDINGS: WordingFile = Either.getOrThrow(
  parseSchema(WordingFileSchema, data)
);

export const TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES: readonly ToolCallSchemaFuzzCase[] =
  realisticCases(TOOLCALL_SCHEMA_FUZZ_WORDINGS);
