import type { Arbitrary } from "fast-check";
import {
  array,
  boolean,
  constant,
  dictionary,
  double,
  jsonValue,
  maxSafeInteger,
  oneof,
  string,
  stringMatching,
} from "fast-check";

import type { ValueOf } from "../../internal/guards";
import type { JsonSchema } from "./json-schema";

export type Defs = Readonly<Record<string, JsonSchema>>;

export interface Built {
  readonly schema: JsonSchema;
  readonly defs: Defs;
  readonly values: Arbitrary<unknown>;
}

export interface Shape {
  readonly key: string;
  readonly strictOk: boolean;
  readonly built: Arbitrary<Built>;
  readonly control?: string;
  readonly root?: true;
}

export const PRIMITIVES = [
  "string",
  "integer",
  "number",
  "boolean",
  "null",
] as const;
export type Primitive = ValueOf<typeof PRIMITIVES>;

export const strings: Arbitrary<string> = oneof(
  string({ unit: "grapheme", maxLength: 24 }),
  string({ unit: "binary-ascii", maxLength: 24 })
);
const numbers: Arbitrary<number> = double({
  noNaN: true,
  noDefaultInfinity: true,
}).filter((value) => !Object.is(value, -0));
export const fractions: Arbitrary<number> = numbers.filter(
  (value) => !Number.isInteger(value)
);
export const VALUES: Readonly<Record<Primitive, Arbitrary<unknown>>> = {
  string: strings,
  integer: maxSafeInteger(),
  number: numbers,
  boolean: boolean(),
  null: constant(null),
};
export const propertyKeys: Arbitrary<string> = stringMatching(
  /^[A-Za-z_][\w-]{0,15}$/
);
export const ANY_VALUES: Readonly<Record<string, Arbitrary<unknown>>> = {
  ...VALUES,
  object: dictionary(propertyKeys, jsonValue({ maxDepth: 1 }), {
    minKeys: 1,
    maxKeys: 3,
  }),
  array: array(jsonValue({ maxDepth: 1 }), { minLength: 1, maxLength: 3 }),
};

export function fixed(
  schema: JsonSchema,
  values: Arbitrary<unknown>
): Arbitrary<Built> {
  return constant({ schema, defs: {}, values });
}

export function hash(text: string): number {
  let value = 2_166_136_261;
  for (let index = 0; index < text.length; index += 1) {
    value = Math.imul(value ^ text.charCodeAt(index), 16_777_619) >>> 0;
  }
  return value;
}
