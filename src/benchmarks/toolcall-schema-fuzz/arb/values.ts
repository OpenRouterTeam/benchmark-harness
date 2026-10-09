import type { Arbitrary } from "fast-check";
import {
  array,
  base64String,
  boolean,
  constant,
  date,
  dictionary,
  domain,
  double,
  emailAddress,
  ipV4,
  letrec,
  maxSafeInteger,
  oneof,
  string,
  stringMatching,
  uuid,
  webUrl,
} from "fast-check";

import type { JsonType } from "./node";

const MAX_STRING_LENGTH = 24;
const MAX_CHILDREN = 4;
const MAX_VALUE_DEPTH = 3;

const isoDates: Arbitrary<string> = date({
  noInvalidDate: true,
  min: new Date("1900-01-01T00:00:00.000Z"),
  max: new Date("2199-12-31T23:59:59.999Z"),
}).map((value) => value.toISOString());

const stringValues: Arbitrary<string> = oneof(
  {
    arbitrary: string({ unit: "grapheme", maxLength: MAX_STRING_LENGTH }),
    weight: 4,
  },
  {
    arbitrary: string({ unit: "binary-ascii", maxLength: MAX_STRING_LENGTH }),
    weight: 2,
  },
  isoDates,
  isoDates.map((value) => value.slice(0, 10)),
  isoDates.map((value) => value.slice(11)),
  emailAddress(),
  uuid(),
  webUrl(),
  ipV4(),
  domain(),
  { arbitrary: base64String({ minLength: 4, maxLength: 16 }), weight: 3 }
);

const numbers: Arbitrary<number> = double({
  noNaN: true,
  noDefaultInfinity: true,
}).filter((value) => !Object.is(value, -0));
const fractions: Arbitrary<number> = numbers.filter(
  (value) => !Number.isInteger(value)
);

export const keys: Arbitrary<string> = oneof(
  { arbitrary: stringMatching(/^[A-Za-z_][\w-]{0,15}$/), weight: 4 },
  string({ unit: "grapheme", minLength: 1, maxLength: 12 })
).filter((key) => key !== "__proto__");

export const scalarValues: Arbitrary<unknown> = oneof(
  stringValues,
  maxSafeInteger(),
  numbers,
  boolean(),
  constant(null)
);

const tree = letrec<{
  value: unknown;
  array: unknown[];
  object: Record<string, unknown>;
}>((tie) => ({
  value: oneof(
    { depthSize: "small", maxDepth: MAX_VALUE_DEPTH },
    { arbitrary: scalarValues, weight: 3 },
    tie("array"),
    tie("object")
  ),
  array: array(tie("value"), { maxLength: MAX_CHILDREN }),
  object: dictionary(keys, tie("value"), { maxKeys: MAX_CHILDREN }),
}));

export const jsonValues: Arbitrary<unknown> = tree.value;

export const argumentValues: Arbitrary<Record<string, unknown>> = dictionary(
  keys,
  jsonValues,
  {
    minKeys: 1,
    maxKeys: 5,
  }
);

export const VALUES_OF_TYPE: Readonly<Record<JsonType, Arbitrary<unknown>>> = {
  string: stringValues,
  integer: maxSafeInteger(),
  number: fractions,
  boolean: boolean(),
  null: constant(null),
  array: array(scalarValues, { maxLength: 2 }),
  object: dictionary(keys, scalarValues, { minKeys: 1, maxKeys: 2 }),
};
