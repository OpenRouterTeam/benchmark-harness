import type { Arbitrary } from "fast-check";
import {
  array,
  base64String,
  constant,
  constantFrom,
  date,
  dictionary,
  double,
  emailAddress,
  integer,
  letrec,
  maxSafeInteger,
  nat,
  oneof,
  stringMatching,
  tuple,
  uniqueArray,
  uuid,
  webUrl,
} from "fast-check";

import type { Built, Shape } from "./shape";
import { fixed, PRIMITIVES, propertyKeys, strings, VALUES } from "./shape";

const nonEmptyStrings: Arbitrary<string> = strings.filter(
  (value) => value.length > 0
);
const ISO_DATES: Arbitrary<Date> = date({
  noInvalidDate: true,
  min: new Date("1900-01-01T00:00:00.000Z"),
  max: new Date("2199-12-31T23:59:59.999Z"),
});

function escapeRegex(text: string): string {
  return text.replaceAll(/[$()*+./?[\\\]^{|}]/g, String.raw`\$&`);
}

function codePoints(text: string): number {
  return [...text].length;
}

function derived(
  values: Arbitrary<unknown>,
  schemaOf: (value: unknown) => Built["schema"]
): Arbitrary<Built> {
  return values.map((value) => ({
    schema: schemaOf(value),
    defs: {},
    values: constant(value),
  }));
}

function leaf({
  key,
  built,
  strictOk,
  control,
}: {
  readonly key: string;
  readonly built: Arbitrary<Built>;
  readonly strictOk: boolean;
  readonly control?: string;
}): Shape {
  return {
    key,
    strictOk,
    built,
    ...(control === undefined ? {} : { control }),
  };
}

const FORMATS: Readonly<Record<string, Arbitrary<string>>> = {
  "date-time": ISO_DATES.map((value) => value.toISOString()),
  date: ISO_DATES.map((value) => value.toISOString().slice(0, 10)),
  email: emailAddress(),
  uuid: uuid(),
  uri: webUrl({ withQueryParameters: true, withFragments: true }),
};

const TREE_DEF = "node";
const TREE_SCHEMA = {
  type: "object",
  properties: {
    label: { type: "string" },
    children: { type: "array", items: { $ref: `#/$defs/${TREE_DEF}` } },
  },
  required: ["label", "children"],
  additionalProperties: false,
} as const;
const trees: Arbitrary<unknown> = letrec<{ node: unknown }>((tie) => ({
  node: tuple(
    strings,
    oneof(
      { maxDepth: 2, depthSize: "small" },
      constant([]),
      array(tie("node"), { maxLength: 2 })
    )
  ).map(([label, children]) => ({ label, children })),
})).node;

const bounded = tuple(maxSafeInteger(), nat(1000), nat(1000));

function pairedObject(
  keywordOf: (trigger: string, dependent: string) => Built["schema"]
): Arbitrary<Built> {
  return uniqueArray(propertyKeys, { minLength: 2, maxLength: 2 }).map(
    ([a = "a", b = "b"]) => ({
      schema: {
        type: "object",
        properties: { [a]: { type: "string" }, [b]: { type: "string" } },
        ...keywordOf(a, b),
      },
      defs: {},
      values: tuple(strings, strings).map(([x, y]) => ({ [a]: x, [b]: y })),
    })
  );
}

const SPEC_LEAVES: readonly Shape[] = [
  leaf({
    key: "not<string>@integer",
    built: fixed({ not: { type: "string" } }, VALUES.integer),
    strictOk: false,
    control: "integer",
  }),
  leaf({
    key: "if<integer>then{minimum}else<string>@integer",
    built: derived(maxSafeInteger(), (value) => ({
      if: { type: "integer" },
      // oxlint-disable-next-line unicorn/no-thenable -- `then` is the JSON Schema conditional keyword, not a thenable
      then: { type: "integer", minimum: Number(value) - 10 },
      else: { type: "string" },
    })),
    strictOk: false,
    control: "integer",
  }),
  leaf({
    key: "array{contains<integer>}",
    built: fixed(
      {
        type: "array",
        items: { type: ["string", "integer"] },
        contains: { type: "integer" },
        minContains: 1,
        maxContains: 2,
      },
      tuple(strings, maxSafeInteger())
    ),
    strictOk: false,
    control: "array/string",
  }),
  leaf({
    key: "array{uniqueItems}<string>",
    built: fixed(
      { type: "array", items: { type: "string" }, uniqueItems: true },
      uniqueArray(strings, { minLength: 1, maxLength: 3 })
    ),
    strictOk: false,
    control: "array/string",
  }),
  leaf({
    key: "map{patternProperties}<integer>",
    built: fixed(
      {
        type: "object",
        patternProperties: { "^x_": { type: "integer" } },
        additionalProperties: false,
      },
      dictionary(
        propertyKeys.map((key) => `x_${key}`),
        maxSafeInteger(),
        { minKeys: 1, maxKeys: 3 }
      )
    ),
    strictOk: false,
    control: "map<integer>",
  }),
  leaf({
    key: "map{propertyNames,minProperties,maxProperties}<string>",
    built: fixed(
      {
        type: "object",
        propertyNames: { pattern: "^[a-z_]+$" },
        additionalProperties: { type: "string" },
        minProperties: 1,
        maxProperties: 3,
      },
      dictionary(stringMatching(/^[_a-z]{1,12}$/), strings, {
        minKeys: 1,
        maxKeys: 3,
      })
    ),
    strictOk: false,
    control: "map<string>",
  }),
  leaf({
    key: "object{dependentRequired}",
    built: pairedObject((trigger, dependent) => ({
      dependentRequired: { [trigger]: [dependent] },
    })),
    strictOk: false,
  }),
  leaf({
    key: "object{dependentSchemas}",
    built: pairedObject((trigger, dependent) => ({
      dependentSchemas: { [trigger]: { required: [dependent] } },
    })),
    strictOk: false,
  }),
  leaf({
    key: "string{annotations}",
    built: strings.map((fallback) => ({
      schema: {
        type: "string",
        title: "Value",
        default: fallback,
        examples: [fallback],
        deprecated: false,
        readOnly: false,
        writeOnly: false,
        $comment: "generated",
      },
      defs: {},
      values: strings,
    })),
    strictOk: false,
    control: "string",
  }),
  leaf({
    key: "string{contentEncoding=base64}",
    built: fixed(
      {
        type: "string",
        contentEncoding: "base64",
        contentMediaType: "application/octet-stream",
      },
      base64String({ minLength: 4, maxLength: 24 })
    ),
    strictOk: false,
    control: "string",
  }),
];

export const KEYWORD_LEAVES: readonly Shape[] = [
  leaf({
    key: "string{minLength,maxLength}",
    built: derived(nonEmptyStrings, (value) => {
      const length = codePoints(String(value));
      return {
        type: "string",
        minLength: Math.ceil(length / 2),
        maxLength: length,
      };
    }),
    strictOk: false,
    control: "string",
  }),
  leaf({
    key: "string{pattern=literal}",
    built: derived(strings, (value) => ({
      type: "string",
      pattern: `^${escapeRegex(String(value))}$`,
    })),
    strictOk: true,
    control: "string",
  }),
  leaf({
    key: "string{pattern=class}",
    built: fixed(
      { type: "string", pattern: "^[A-Za-z0-9_-]+$" },
      stringMatching(/^[\w-]{1,24}$/)
    ),
    strictOk: true,
    control: "string",
  }),
  ...Object.entries(FORMATS).map(([format, values]) =>
    leaf({
      key: `string{format=${format}}`,
      built: fixed({ type: "string", format }, values),
      strictOk: true,
      control: "string",
    })
  ),
  leaf({
    key: "string{description}",
    built: strings.map((description) => ({
      schema: { type: "string", description },
      defs: {},
      values: strings,
    })),
    strictOk: true,
    control: "string",
  }),
  leaf({
    key: "integer{minimum,maximum}",
    built: bounded.map(([value, below, above]) => ({
      schema: {
        type: "integer",
        minimum: value - below,
        maximum: value + above,
      },
      defs: {},
      values: constant(value),
    })),
    strictOk: true,
    control: "integer",
  }),
  leaf({
    key: "integer{multipleOf}",
    built: tuple(
      integer({ min: 2, max: 1000 }),
      integer({ min: -1_000_000, max: 1_000_000 })
    ).map(([step, factor]) => ({
      schema: { type: "integer", multipleOf: step },
      defs: {},
      values: constant(step * factor),
    })),
    strictOk: true,
    control: "integer",
  }),
  leaf({
    key: "number{exclusiveMinimum,exclusiveMaximum}",
    built: derived(
      double({ min: -1_000_000, max: 1_000_000, noNaN: true }).filter(
        (value) => !Number.isInteger(value)
      ),
      (value) => ({
        type: "number",
        exclusiveMinimum: Math.floor(Number(value)),
        exclusiveMaximum: Math.ceil(Number(value)),
      })
    ),
    strictOk: true,
    control: "number",
  }),
  leaf({
    key: "array{minItems,maxItems}<string>",
    built: fixed(
      { type: "array", items: { type: "string" }, minItems: 1, maxItems: 3 },
      array(strings, { minLength: 1, maxLength: 3 })
    ),
    strictOk: true,
    control: "array/string",
  }),
  leaf({
    key: "prefixItems[string,integer,boolean]",
    built: fixed(
      {
        type: "array",
        prefixItems: [
          { type: "string" },
          { type: "integer" },
          { type: "boolean" },
        ],
        items: false,
      },
      tuple(VALUES.string, VALUES.integer, VALUES.boolean)
    ),
    strictOk: false,
  }),
  ...PRIMITIVES.map((type) =>
    leaf({
      key: `map<${type}>`,
      built: fixed(
        { type: "object", additionalProperties: { type } },
        dictionary(propertyKeys, VALUES[type], { minKeys: 1, maxKeys: 3 })
      ),
      strictOk: false,
      control: type,
    })
  ),
  leaf({
    key: "enum<mixed>",
    built: uniqueArray(oneof(...PRIMITIVES.map((type) => VALUES[type])), {
      minLength: 2,
      maxLength: 4,
      selector: (value) => JSON.stringify(value),
    }).map((members) => ({
      schema: { enum: members },
      defs: {},
      values: constantFrom(...members),
    })),
    strictOk: true,
  }),
  leaf({
    key: "tree",
    built: constant({
      schema: { $ref: `#/$defs/${TREE_DEF}` },
      defs: { [TREE_DEF]: TREE_SCHEMA },
      values: trees,
    }),
    strictOk: true,
  }),
  ...SPEC_LEAVES,
];
