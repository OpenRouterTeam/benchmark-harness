import type { Arbitrary } from "fast-check";
import { boolean, tuple } from "fast-check";

import { isRecord } from "../../internal/guards";
import { describeRoot } from "./arb/describe";
import type { SchemaNode } from "./arb/node";
import { parametersOf } from "./arb/node";
import { argumentValues } from "./arb/values";
import { validateJsonSchema } from "./json-schema";
import { roundTripJson, toolNames } from "./tool-case";

export interface Draw {
  readonly name: string;
  readonly strict: boolean;
  readonly node: SchemaNode;
  readonly args: Record<string, unknown>;
}

export { parametersOf };

export const randomDraws: Arbitrary<Draw> = tuple(
  toolNames,
  boolean(),
  argumentValues.map(roundTripJson).filter(isRecord)
)
  .chain(([name, strict, args]) =>
    describeRoot(args, strict).map((node) => ({ name, strict, node, args }))
  )
  .filter(
    (draw) =>
      validateJsonSchema(draw.args, parametersOf(draw.node)).length === 0
  );
