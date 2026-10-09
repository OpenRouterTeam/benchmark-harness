import type { Arbitrary } from "fast-check";
import {
  array,
  boolean,
  constant,
  constantFrom,
  sample,
  string,
  tuple,
  uniqueArray,
} from "fast-check";

import type { ValueOf } from "../../internal/guards";
import { isRecord } from "../../internal/guards";
import type { JsonSchema } from "./json-schema";
import { validateJsonSchema } from "./json-schema";
import { KEYWORD_LEAVES } from "./keywords";
import type { Built, Primitive, Shape } from "./shape";
import {
  ANY_VALUES,
  fixed,
  fractions,
  hash,
  PRIMITIVES,
  propertyKeys,
  strings,
  VALUES,
} from "./shape";
import type { FuzzScenario } from "./tool-case";
import {
  FuzzScenario as Scenario,
  roundTripJson,
  toolNames,
} from "./tool-case";

interface Wrapper {
  readonly name: string;
  readonly strictOk: boolean;
  readonly wrap: (inner: Built) => Arbitrary<Built>;
}

const UNIONS = ["anyOf", "oneOf"] as const;

const ORDERED_PAIRS: readonly (readonly [Primitive, Primitive])[] =
  PRIMITIVES.flatMap((a) =>
    PRIMITIVES.filter((b) => b !== a).map((b) => [a, b] as const)
  );

function branchValues({
  keyword,
  pick,
  other,
}: {
  readonly keyword: ValueOf<typeof UNIONS>;
  readonly pick: Primitive;
  readonly other: Primitive;
}): Arbitrary<unknown> | undefined {
  if (keyword === "oneOf" && pick === "integer" && other === "number") {
    return undefined;
  }
  if (keyword === "oneOf" && pick === "number" && other === "integer") {
    return fractions;
  }
  return VALUES[pick];
}

const primitiveLeaves: readonly Shape[] = PRIMITIVES.map((type) => ({
  key: type,
  strictOk: true,
  built: fixed({ type }, VALUES[type]),
}));

const singleTypeLeaves: readonly Shape[] = [
  ...primitiveLeaves,
  ...PRIMITIVES.map((type) => ({
    key: `enum<${type}>`,
    strictOk: true,
    control: type,
    built: uniqueArray(VALUES[type], {
      minLength: 1,
      maxLength: 3,
      selector: (value) => JSON.stringify(value),
    }).map((members) => ({
      schema: { enum: members },
      defs: {},
      values: constantFrom(...members),
    })),
  })),
  ...PRIMITIVES.map((type) => ({
    key: `const<${type}>`,
    strictOk: false,
    control: type,
    built: VALUES[type].map((value) => ({
      schema: { const: value },
      defs: {},
      values: constant(value),
    })),
  })),
  ...Object.entries(ANY_VALUES).map(([kind, values]) => ({
    key: `{}@${kind}`,
    strictOk: false,
    ...(kind in VALUES ? { control: kind } : {}),
    built: fixed({}, values),
  })),
];

const pairLeaves: readonly Shape[] = ORDERED_PAIRS.flatMap(([a, b]) => [
  ...[a, b].map((pick) => ({
    key: `type[${a},${b}]@${pick}`,
    strictOk: true,
    control: pick,
    built: fixed({ type: [a, b] }, VALUES[pick]),
  })),
  ...UNIONS.flatMap((keyword) =>
    [a, b].flatMap((pick) => {
      const values = branchValues({ keyword, pick, other: pick === a ? b : a });
      return values === undefined
        ? []
        : [
            {
              key: `${keyword}[${a},${b}]@${pick}`,
              strictOk: keyword === "anyOf",
              control: pick,
              built: fixed({ [keyword]: [{ type: a }, { type: b }] }, values),
            },
          ];
    })
  ),
]);

function field({
  required,
  closed,
  keys = propertyKeys,
  name = `obj(${required ? "req" : "opt"},${closed ? "closed" : "open"})`,
}: {
  readonly required: boolean;
  readonly closed: boolean;
  readonly keys?: Arbitrary<string>;
  readonly name?: string;
}): Wrapper {
  return {
    name,
    strictOk: required && closed,
    wrap: (inner) =>
      keys.map((key) => ({
        schema: {
          type: "object",
          properties: { [key]: inner.schema },
          required: required ? [key] : [],
          ...(closed ? { additionalProperties: false } : {}),
        },
        defs: inner.defs,
        values: inner.values.map((value) => ({ [key]: value })),
      })),
  };
}

function nullable({
  nullFirst,
  pickNull,
}: {
  readonly nullFirst: boolean;
  readonly pickNull: boolean;
}): Wrapper {
  return {
    name: `${nullFirst ? "nullable-first" : "nullable"}${pickNull ? "@null" : ""}`,
    strictOk: true,
    wrap: (inner) =>
      constant({
        schema: {
          anyOf: nullFirst
            ? [{ type: "null" }, inner.schema]
            : [inner.schema, { type: "null" }],
        },
        defs: inner.defs,
        values: pickNull ? constant(null) : inner.values,
      }),
  };
}

const ROOT = field({ required: true, closed: true });

const WRAPPERS: readonly Wrapper[] = [
  ROOT,
  field({ required: true, closed: false }),
  field({ required: false, closed: true }),
  field({ required: false, closed: false }),
  field({
    required: true,
    closed: true,
    keys: string({ unit: "grapheme", minLength: 1, maxLength: 12 }),
    name: "obj(any-key)",
  }),
  {
    name: "array",
    strictOk: true,
    wrap: (inner) =>
      constant({
        schema: { type: "array", items: inner.schema },
        defs: inner.defs,
        values: array(inner.values, { minLength: 1, maxLength: 3 }),
      }),
  },
  {
    name: "ref",
    strictOk: true,
    wrap: (inner) => {
      const name = `def_${hash(JSON.stringify(inner.schema)).toString(36)}`;
      return constant({
        schema: { $ref: `#/$defs/${name}` },
        defs: { ...inner.defs, [name]: inner.schema },
        values: inner.values,
      });
    },
  },
  nullable({ nullFirst: false, pickNull: false }),
  nullable({ nullFirst: true, pickNull: false }),
  nullable({ nullFirst: false, pickNull: true }),
  nullable({ nullFirst: true, pickNull: true }),
  {
    name: "allOf",
    strictOk: false,
    wrap: (inner) =>
      uniqueArray(propertyKeys, { minLength: 2, maxLength: 2 }).map(
        ([a = "a", b = "b"]) => ({
          schema: {
            allOf: [
              {
                type: "object",
                properties: { [a]: inner.schema },
                required: [a],
              },
              {
                type: "object",
                properties: { [b]: { type: "string" } },
                required: [b],
              },
            ],
          },
          defs: inner.defs,
          values: tuple(inner.values, strings).map(([x, y]) => ({
            [a]: x,
            [b]: y,
          })),
        })
      ),
  },
];

function refName(schema: JsonSchema): string {
  return hash(JSON.stringify(schema)).toString(36);
}

const REF_FORMS: readonly Wrapper[] = [
  {
    name: "ref+description",
    strictOk: true,
    wrap: (inner) =>
      strings.map((description) => {
        const name = `def_${refName(inner.schema)}`;
        return {
          schema: { $ref: `#/$defs/${name}`, description },
          defs: { ...inner.defs, [name]: inner.schema },
          values: inner.values,
        };
      }),
  },
  {
    name: "ref+default",
    strictOk: false,
    wrap: (inner) =>
      inner.values.map(roundTripJson).map((fallback) => {
        const name = `def_${refName(inner.schema)}`;
        return {
          schema: { $ref: `#/$defs/${name}`, default: fallback },
          defs: { ...inner.defs, [name]: inner.schema },
          values: inner.values,
        };
      }),
  },
  {
    name: "ref~escaped",
    strictOk: true,
    wrap: (inner) => {
      const name = `api/Model~v1 ${refName(inner.schema)}`;
      return constant({
        schema: { $ref: `#/$defs/api~1Model~0v1%20${refName(inner.schema)}` },
        defs: { ...inner.defs, [name]: inner.schema },
        values: inner.values,
      });
    },
  },
  {
    name: "allOf1",
    strictOk: false,
    wrap: (inner) =>
      constant({
        schema: { allOf: [inner.schema] },
        defs: inner.defs,
        values: inner.values,
      }),
  },
];

function compositeTypes(
  type: "object" | "array",
  nullFirst: boolean
): readonly string[] {
  return nullFirst ? ["null", type] : [type, "null"];
}

function nullableComposites({
  nullFirst,
  pickNull,
}: {
  readonly nullFirst: boolean;
  readonly pickNull: boolean;
}): readonly Wrapper[] {
  const order = (type: string): string =>
    nullFirst ? `null,${type}` : `${type},null`;
  const suffix = pickNull ? "@null" : "";
  return [
    {
      name: `type[${order("object")}]${suffix}`,
      strictOk: true,
      wrap: (inner) =>
        propertyKeys.map((key) => ({
          schema: {
            type: compositeTypes("object", nullFirst),
            properties: { [key]: inner.schema },
            required: [key],
            additionalProperties: false,
          },
          defs: inner.defs,
          values: pickNull
            ? constant(null)
            : inner.values.map((value) => ({ [key]: value })),
        })),
    },
    {
      name: `type[${order("array")}]${suffix}`,
      strictOk: true,
      wrap: (inner) =>
        constant({
          schema: {
            type: compositeTypes("array", nullFirst),
            items: inner.schema,
          },
          defs: inner.defs,
          values: pickNull
            ? constant(null)
            : array(inner.values, { minLength: 1, maxLength: 3 }),
        }),
    },
  ];
}

const OTHER_BRANCHES: readonly JsonSchema[] = [
  { type: "array", items: { type: "string" } },
  { type: "object" },
];

const CARDINALITY_FORMS: readonly Wrapper[] = [
  ...UNIONS.map((keyword) => ({
    name: `${keyword}1`,
    strictOk: keyword === "anyOf",
    wrap: (inner: Built) =>
      constant({
        schema: { [keyword]: [inner.schema] },
        defs: inner.defs,
        values: inner.values,
      }),
  })),
  ...UNIONS.map((keyword) => ({
    name: `${keyword}3`,
    strictOk: false,
    wrap: (inner: Built) =>
      constant({
        schema: {
          [keyword]: [OTHER_BRANCHES[0], inner.schema, OTHER_BRANCHES[1]],
        },
        defs: inner.defs,
        values: inner.values,
      }),
  })),
  {
    name: "oneOf3-tagged",
    strictOk: false,
    wrap: (inner) =>
      uniqueArray(propertyKeys, { minLength: 2, maxLength: 2 }).map(
        ([tag = "kind", key = "value"]) => {
          const branch = (kind: string, schema: JsonSchema): JsonSchema => ({
            type: "object",
            properties: { [tag]: { const: kind }, [key]: schema },
            required: [tag, key],
          });
          return {
            schema: {
              oneOf: [
                branch("a", { type: "string" }),
                branch("b", inner.schema),
                branch("c", { type: "array" }),
              ],
            },
            defs: inner.defs,
            values: inner.values.map((value) => ({ [tag]: "b", [key]: value })),
          };
        }
      ),
  },
  {
    name: "allOf3",
    strictOk: false,
    wrap: (inner) =>
      uniqueArray(propertyKeys, { minLength: 3, maxLength: 3 }).map(
        ([a = "a", b = "b", c = "c"]) => ({
          schema: {
            allOf: [
              {
                type: "object",
                properties: { [a]: inner.schema },
                required: [a],
              },
              {
                type: "object",
                properties: { [b]: { type: "string" } },
                required: [b],
              },
              {
                type: "object",
                properties: { [c]: { type: "boolean" } },
                required: [c],
              },
            ],
          },
          defs: inner.defs,
          values: tuple(inner.values, strings, boolean()).map(([x, y, z]) => ({
            [a]: x,
            [b]: y,
            [c]: z,
          })),
        })
      ),
  },
];

const REAL_WORLD_FORMS: readonly Wrapper[] = [
  ...[false, true].flatMap((nullFirst) =>
    [false, true].flatMap((pickNull) =>
      nullFirst && pickNull ? [] : nullableComposites({ nullFirst, pickNull })
    )
  ),
  {
    name: "obj(open-true)",
    strictOk: false,
    wrap: (inner) =>
      propertyKeys.map((key) => ({
        schema: {
          type: "object",
          properties: { [key]: inner.schema },
          required: [key],
          additionalProperties: true,
        },
        defs: inner.defs,
        values: inner.values.map((value) => ({ [key]: value })),
      })),
  },
  ...CARDINALITY_FORMS,
];

const DICT_LEAF: Shape = {
  key: "dict@object",
  strictOk: false,
  built: fixed(
    { type: "object", additionalProperties: true },
    ANY_VALUES["object"] ?? constant({})
  ),
};

function closedObject(key: string, schema: JsonSchema): JsonSchema {
  return {
    type: "object",
    properties: { [key]: schema },
    required: [key],
    additionalProperties: false,
  };
}

interface RootForm {
  readonly name: string;
  readonly build: (parts: {
    readonly own: JsonSchema;
    readonly other: JsonSchema;
    readonly inner: Built;
  }) => {
    readonly schema: JsonSchema;
    readonly defs: Built["defs"];
  };
}

const ROOT_FORMS: readonly RootForm[] = [
  {
    name: "root-anyOf",
    build: ({ own, other, inner }) => ({
      schema: { anyOf: [own, other] },
      defs: inner.defs,
    }),
  },
  {
    name: "root-oneOf",
    build: ({ own, other, inner }) => ({
      schema: { oneOf: [own, other] },
      defs: inner.defs,
    }),
  },
  {
    name: "root-object+anyOf",
    build: ({ own, other, inner }) => ({
      schema: { type: "object", anyOf: [own, other] },
      defs: inner.defs,
    }),
  },
  {
    name: "root-allOf1",
    build: ({ own, inner }) => ({ schema: { allOf: [own] }, defs: inner.defs }),
  },
  {
    name: "root-open-true",
    build: ({ own, inner }) => ({
      schema: { ...own, additionalProperties: true },
      defs: inner.defs,
    }),
  },
  {
    name: "root-ref",
    build: ({ own, inner }) => ({
      schema: { $ref: "#/$defs/Parameters" },
      defs: { ...inner.defs, Parameters: own },
    }),
  },
];

function rootShape(form: RootForm, inner: Shape): Shape {
  return {
    key: `${form.name}/${inner.key}`,
    strictOk: false,
    control: inner.key,
    root: true,
    built: inner.built.chain((built) =>
      uniqueArray(propertyKeys, { minLength: 2, maxLength: 2 }).map(
        ([key = "a", otherKey = "b"]) => ({
          ...form.build({
            own: closedObject(key, built.schema),
            other: closedObject(otherKey, { type: "string" }),
            inner: built,
          }),
          values: built.values.map((value) => ({ [key]: value })),
        })
      )
    ),
  };
}

function definitionsShape(inner: Shape): Shape {
  return {
    key: `definitions/${inner.key}`,
    strictOk: false,
    control: inner.key,
    root: true,
    built: inner.built.chain((built) =>
      propertyKeys.map((key) => {
        const name = `Def${refName(built.schema)}`;
        return {
          schema: {
            ...closedObject(key, { $ref: `#/definitions/${name}` }),
            definitions: { [name]: built.schema },
          },
          defs: built.defs,
          values: built.values.map((value) => ({ [key]: value })),
        };
      })
    ),
  };
}

function wrapShape(wrapper: Wrapper, inner: Shape): Shape {
  return {
    key: `${wrapper.name}/${inner.key}`,
    strictOk: wrapper.strictOk && inner.strictOk,
    control: inner.key,
    built: inner.built.chain(wrapper.wrap),
  };
}

function wrapAll(shapes: readonly Shape[]): readonly Shape[] {
  return WRAPPERS.flatMap((wrapper) =>
    shapes.map((shape) => wrapShape(wrapper, shape))
  );
}

const LEAVES: readonly Shape[] = [
  ...singleTypeLeaves,
  ...pairLeaves,
  ...KEYWORD_LEAVES,
];

const SINGLE_SHAPES: readonly Shape[] = [
  ...LEAVES,
  ...wrapAll(singleTypeLeaves),
  ...wrapAll(wrapAll(primitiveLeaves)),
  ...REF_FORMS.flatMap((wrapper) =>
    singleTypeLeaves.map((shape) => wrapShape(wrapper, shape))
  ),
  ...singleTypeLeaves.map(definitionsShape),
  ...ROOT_FORMS.flatMap((form) =>
    primitiveLeaves.map((shape) => rootShape(form, shape))
  ),
  ...REAL_WORLD_FORMS.flatMap((wrapper) =>
    primitiveLeaves.map((shape) => wrapShape(wrapper, shape))
  ),
  DICT_LEAF,
];

const MULTI_MESSAGE_SCENARIOS = [
  Scenario.Replay,
  Scenario.Distractor,
  Scenario.Parallel,
] as const;

export interface Slot {
  readonly shape: Shape;
  readonly scenario: FuzzScenario;
  readonly strict: boolean;
}

export const GRID_SLOTS: readonly Slot[] = [
  ...SINGLE_SHAPES.map((shape) => ({ shape, scenario: Scenario.Single })),
  ...MULTI_MESSAGE_SCENARIOS.flatMap((scenario) =>
    LEAVES.map((shape) => ({ shape, scenario }))
  ),
].flatMap(({ shape, scenario }) =>
  (shape.strictOk ? [false, true] : [false]).map((strict) => ({
    shape,
    scenario,
    strict,
  }))
);

const ID_PREFIX = "toolcall_schema_fuzz-grid:";

export function slotCaseId({
  prefix,
  slot,
  draw,
}: {
  readonly prefix: string;
  readonly slot: Slot;
  readonly draw: number;
}): string {
  const mode = slot.strict ? "strict" : "loose";
  const scope = slot.scenario === Scenario.Single ? "" : `${slot.scenario}:`;
  return `${prefix}${mode}:${scope}${slot.shape.key}${draw === 0 ? "" : `#${draw}`}`;
}

function caseId(slot: Slot, draw: number): string {
  return slotCaseId({ prefix: ID_PREFIX, slot, draw });
}

export interface GridDraw {
  readonly name: string;
  readonly parameters: JsonSchema;
  readonly args: readonly [Record<string, unknown>, Record<string, unknown>];

  readonly isDistinct: boolean;
}

const DISTINCT_ATTEMPTS = 8;

function draw(shape: Shape, seed: number): GridDraw | undefined {
  const root = shape.root === true ? shape : wrapShape(ROOT, shape);
  const [picked] = sample(tuple(toolNames, root.built), { seed, numRuns: 1 });
  if (picked === undefined) {
    return undefined;
  }
  const [name, built] = picked;
  const parameters =
    Object.keys(built.defs).length === 0
      ? built.schema
      : { ...built.schema, $defs: built.defs };
  const valid = built.values
    .map(roundTripJson)
    .filter(isRecord)
    .filter((args) => validateJsonSchema(args, parameters).length === 0);
  const [first, ...rest] = sample(valid, { seed, numRuns: DISTINCT_ATTEMPTS });
  if (first === undefined) {
    return undefined;
  }
  const firstJson = JSON.stringify(first);
  const distinct = rest.find((args) => JSON.stringify(args) !== firstJson);
  const [second = first] = distinct === undefined ? rest : [distinct];
  return {
    name,
    parameters,
    args: [first, second],
    isDistinct: distinct !== undefined,
  };
}

export function gridShapeDraws(): readonly {
  readonly shape: Shape;
  readonly drawn: GridDraw;
}[] {
  return SINGLE_SHAPES.flatMap((shape) => {
    const slot = { shape, scenario: Scenario.Single, strict: false };
    const drawn = draw(shape, hash(caseId(slot, 0)));
    return drawn === undefined ? [] : [{ shape, drawn }];
  });
}
