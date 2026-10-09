import type { Arbitrary } from "fast-check";
import {
  array,
  boolean,
  constant,
  constantFrom,
  nat,
  oneof,
  record,
  shuffledSubarray,
  string,
  stringMatching,
  tuple,
  uniqueArray,
} from "fast-check";

import { isRecord } from "../../../internal/guards";
import type { JsonSchema } from "../json-schema";
import { hash } from "../shape";
import { anyOfNode, arrayNode, objectNode } from "./collections";
import { stringKeywords, numberKeywords } from "./leaves";
import type { Context, SchemaNode } from "./node";
import {
  accepts,
  deeper,
  jsonTypeOf,
  JsonType,
  leaf,
  rejectingTypes,
  withChildren,
} from "./node";
import { jsonValues, scalarValues, VALUES_OF_TYPE } from "./values";

type Weighted = {
  readonly arbitrary: Arbitrary<SchemaNode>;
  readonly weight: number;
};

interface Wrapping {
  readonly inner: Arbitrary<SchemaNode>;
  readonly value: unknown;
}

const MAX_WRAP_DEPTH = 3;
const JSON_VALUE_DEF = "JsonValue";
const JSON_VALUE_REF = `#/$defs/${JSON_VALUE_DEF}`;
const SCHEMA_DIALECT = "https://json-schema.org/draft/2020-12/schema";

const text: Arbitrary<string> = string({ maxLength: 40 });
const plainNames: Arbitrary<string> = stringMatching(/^[A-Z][A-Za-z]{2,11}$/);
const escapedNames: Arbitrary<string> = oneof(
  plainNames,
  plainNames.map((name) => `${name}~v1`),
  plainNames.map((name) => `api/${name}`),
  plainNames.map((name) => `${name} Model`),
  plainNames.map((name) => `${name}%2F`),
  plainNames.map((name) => `models.${name}`)
);

function pointerOf(container: "$defs" | "definitions", name: string): string {
  const token = name.replaceAll("~", "~0").replaceAll("/", "~1");
  return `#/${container}/${encodeURIComponent(token).replaceAll("%7E", "~")}`;
}

function baseNode(value: unknown, context: Context): Arbitrary<SchemaNode> {
  const type = jsonTypeOf(value);
  switch (type) {
    case JsonType.String: {
      return stringKeywords(String(value), context.strict).map((keywords) =>
        leaf({ type, ...keywords })
      );
    }
    case JsonType.Integer:
    case JsonType.Number: {
      const declared =
        type === JsonType.Integer
          ? constantFrom(type, JsonType.Number)
          : constant(type);
      return tuple(declared, numberKeywords(Number(value), context.strict)).map(
        ([name, keywords]) => leaf({ type: name, ...keywords })
      );
    }
    case JsonType.Boolean:
    case JsonType.Null: {
      return constant(leaf({ type }));
    }
    case JsonType.Array: {
      return arrayNode(Array.isArray(value) ? value : [], context);
    }
    case JsonType.Object: {
      return objectNode(isRecord(value) ? value : {}, context);
    }
    default: {
      return type satisfies never;
    }
  }
}

function rejecting(value: unknown, context: Context): Arbitrary<SchemaNode> {
  return constantFrom(...rejectingTypes(value)).chain((type) =>
    VALUES_OF_TYPE[type]
      .chain((other) => context.describe(other, deeper(context, 2)))
      .map((node) => (accepts(value, node) ? leaf({ type }) : node))
  );
}

function other(context: Context): Arbitrary<SchemaNode> {
  return jsonValues.chain((value) =>
    context.describe(value, deeper(context, 2))
  );
}

function isScalar(value: unknown): boolean {
  return !Array.isArray(value) && !isRecord(value);
}

function alternatives(
  value: unknown,
  context: Context
): readonly Arbitrary<SchemaNode>[] {
  const own = jsonTypeOf(value);
  const scalar = isScalar(value);
  const scalarTypes = rejectingTypes(value).filter(
    (type) => type !== JsonType.Array && type !== JsonType.Object
  );
  const typeArray = uniqueArray(constantFrom(...scalarTypes), {
    minLength: 1,
    maxLength: 3,
  }).chain((extra) =>
    shuffledSubarray([own, ...extra], { minLength: extra.length + 1 }).map(
      (types) => leaf({ type: types })
    )
  );
  const enumNode = uniqueArray(scalarValues, {
    maxLength: 3,
    selector: (item) => JSON.stringify(item),
  })
    .map((items) =>
      items.filter((item) => JSON.stringify(item) !== JSON.stringify(value))
    )
    .chain((items) =>
      shuffledSubarray([value, ...items], { minLength: items.length + 1 })
    )
    .map((items) => leaf({ enum: items }));
  return [
    ...(scalar ? [typeArray] : []),
    ...(scalar || !context.strict ? [enumNode] : []),
    ...(context.strict
      ? []
      : [constant(leaf({ const: value })), constant(leaf({}))]),
  ];
}

function refSiblings(value: unknown, context: Context): Arbitrary<JsonSchema> {
  const model = context.strict
    ? { description: text }
    : {
        description: text,
        title: text,
        default: constant(value),
        examples: constant([value]),
        deprecated: boolean(),
        $comment: text,
        type: constant(jsonTypeOf(value)),
      };
  return record(model, { requiredKeys: [] });
}

function refNode(
  { inner, value }: Wrapping,
  context: Context
): Arbitrary<SchemaNode> {
  const container = context.strict
    ? constant("$defs" as const)
    : constantFrom("$defs" as const, "definitions" as const);
  return tuple(
    inner,
    container,
    context.strict ? plainNames : escapedNames,
    refSiblings(value, context)
  ).map(([target, where, prefix, siblings]) => {
    const name = `${prefix}${hash(JSON.stringify(target.schema)).toString(36)}`;
    const hoisted = { [name]: target.schema };
    return {
      schema: { $ref: pointerOf(where, name), ...siblings },
      defs: where === "$defs" ? { ...target.defs, ...hoisted } : target.defs,
      definitions:
        where === "definitions"
          ? { ...target.definitions, ...hoisted }
          : target.definitions,
    };
  });
}

const JSON_VALUE_SCHEMA: JsonSchema = {
  anyOf: [
    { type: ["string", "number", "boolean", "null"] },
    { type: "array", items: { $ref: JSON_VALUE_REF } },
    { type: "object", additionalProperties: { $ref: JSON_VALUE_REF } },
  ],
};

const MAX_EXTRA_BRANCHES = 2;

function branchesAround(
  inner: Arbitrary<SchemaNode>,
  others: Arbitrary<SchemaNode>
): Arbitrary<readonly SchemaNode[]> {
  return tuple(
    inner,
    array(others, { maxLength: MAX_EXTRA_BRANCHES }),
    nat()
  ).map(([node, rest, at]) => {
    const index = at % (rest.length + 1);
    return [...rest.slice(0, index), node, ...rest.slice(index)];
  });
}

function nullableComposite([node, nullFirst]: readonly [
  SchemaNode,
  boolean,
]): SchemaNode {
  const type = node.schema["type"];
  if (type !== JsonType.Object && type !== JsonType.Array) {
    return node;
  }
  return {
    ...node,
    schema: {
      ...node.schema,
      type: nullFirst ? [JsonType.Null, type] : [type, JsonType.Null],
    },
  };
}

function strictWrappers(
  { inner, value }: Wrapping,
  context: Context
): readonly Arbitrary<SchemaNode>[] {
  return [
    branchesAround(inner, other(context)).map(anyOfNode),
    tuple(inner, boolean()).map(nullableComposite),
    tuple(inner, boolean()).map(([node, first]) =>
      anyOfNode(
        first ? [node, leaf({ type: "null" })] : [leaf({ type: "null" }), node]
      )
    ),
    refNode({ inner, value }, context),
  ];
}

function withNot(node: SchemaNode, negated: SchemaNode): SchemaNode {
  const schema = "not" in node.schema ? { allOf: [node.schema] } : node.schema;
  return withChildren({ ...schema, not: negated.schema }, [node, negated]);
}

function conditional(
  [node, condition, fallback]: readonly [SchemaNode, SchemaNode, SchemaNode],
  value: unknown
): SchemaNode {
  const holds = accepts(value, condition);
  return withChildren(
    {
      if: condition.schema,
      // oxlint-disable-next-line unicorn/no-thenable -- `then` is the JSON Schema conditional keyword, not a thenable
      then: (holds ? node : fallback).schema,
      else: (holds ? fallback : node).schema,
    },
    [node, condition, fallback]
  );
}

function looseWrappers(
  { inner, value }: Wrapping,
  context: Context
): readonly Arbitrary<SchemaNode>[] {
  const again = context.describe(value, deeper(context));
  return [
    branchesAround(inner, rejecting(value, context)).map((nodes) =>
      withChildren({ oneOf: nodes.map((node) => node.schema) }, nodes)
    ),
    inner.map((node) => withChildren({ allOf: [node.schema] }, [node])),
    tuple(
      inner,
      array(again, { minLength: 1, maxLength: MAX_EXTRA_BRANCHES })
    ).map(([node, rest]) =>
      withChildren({ allOf: [node, ...rest].map((n) => n.schema) }, [
        node,
        ...rest,
      ])
    ),
    tuple(inner, rejecting(value, context)).map(([node, negated]) =>
      withNot(node, negated)
    ),
    tuple(inner, oneof(again, rejecting(value, context)), other(context)).map(
      (parts) => conditional(parts, value)
    ),
    constant({
      schema: { $ref: JSON_VALUE_REF },
      defs: { [JSON_VALUE_DEF]: JSON_VALUE_SCHEMA },
      definitions: {},
    }),
  ];
}

function wrappers(
  { inner, value }: Wrapping,
  context: Context
): readonly Arbitrary<SchemaNode>[] {
  return [
    ...strictWrappers({ inner, value }, context),
    ...(context.strict ? [] : looseWrappers({ inner, value }, context)),
  ];
}

function annotations(value: unknown, context: Context): Arbitrary<JsonSchema> {
  const model = context.strict
    ? { description: text, title: text }
    : {
        description: text,
        title: text,
        default: constant(value),
        examples: constant([value]),
        deprecated: boolean(),
        readOnly: boolean(),
        writeOnly: boolean(),
        $comment: text,
      };
  return oneof(
    { arbitrary: constant({}), weight: 3 },
    { arbitrary: record(model, { requiredKeys: [] }), weight: 1 }
  );
}

function annotate(
  { inner, value }: Wrapping,
  context: Context
): Arbitrary<SchemaNode> {
  return tuple(inner, annotations(value, context)).map(
    ([schemaNode, extra]) => ({
      ...schemaNode,
      schema: { ...schemaNode.schema, ...extra },
    })
  );
}

function weighted(
  base: Arbitrary<SchemaNode>,
  rest: readonly Arbitrary<SchemaNode>[]
): readonly Weighted[] {
  return [
    { arbitrary: base, weight: Math.max(1, Math.ceil(rest.length / 2)) },
    ...rest.map((arbitrary) => ({ arbitrary, weight: 1 })),
  ];
}

function describe(value: unknown, context: Context): Arbitrary<SchemaNode> {
  const inner =
    context.depth < MAX_WRAP_DEPTH
      ? context.describe(value, deeper(context))
      : undefined;
  const rest = [
    ...alternatives(value, context),
    ...(inner === undefined ? [] : wrappers({ inner, value }, context)),
  ];
  return annotate(
    { inner: oneof(...weighted(baseNode(value, context), rest)), value },
    context
  ).map((node) =>
    accepts(value, node) ? node : leaf({ type: jsonTypeOf(value) })
  );
}

function rootExtras(context: Context): Arbitrary<JsonSchema> {
  return context.strict
    ? constant({})
    : record(
        { type: constant("object"), $schema: constant(SCHEMA_DIALECT) },
        { requiredKeys: [] }
      );
}

export function describeRoot(
  value: Readonly<Record<string, unknown>>,
  strict: boolean
): Arbitrary<SchemaNode> {
  const context: Context = { strict, depth: 0, describe };
  const object = objectNode(value, context);
  if (strict) {
    return annotate({ inner: object, value }, context);
  }
  const roots = [
    ...wrappers(
      { inner: objectNode(value, deeper(context)), value },
      context
    ).filter((_, index) => index !== 1 && index !== 2),
    constant(leaf({})),
  ];
  return tuple(
    annotate({ inner: oneof(...weighted(object, roots)), value }, context),
    rootExtras(context)
  ).map(([node, extra]) => ({ ...node, schema: { ...extra, ...node.schema } }));
}
