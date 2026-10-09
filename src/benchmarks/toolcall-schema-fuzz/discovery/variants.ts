import { isRecord } from "../../../internal/guards";
import type { JsonSchema } from "../json-schema";
import { resolveRef, validateJsonSchema } from "../json-schema";

export interface Variant {
  readonly edit: string;
  readonly parameters: JsonSchema;
  readonly args: Readonly<Record<string, unknown>>;
}

interface Node {
  readonly edit: string;
  readonly schema: JsonSchema;
  readonly value: unknown;
}

type Rewrite = (node: Omit<Node, "edit">, root: JsonSchema) => readonly Node[];

function defsOf(root: JsonSchema): JsonSchema {
  return {
    ...(isRecord(root["$defs"]) ? { $defs: root["$defs"] } : {}),
    ...(isRecord(root["definitions"])
      ? { definitions: root["definitions"] }
      : {}),
  };
}

function isValid(
  { value, schema }: { readonly value: unknown; readonly schema: JsonSchema },
  root: JsonSchema
): boolean {
  return validateJsonSchema(value, { ...schema, ...defsOf(root) }).length === 0;
}

function resolve(ref: unknown, root: JsonSchema): JsonSchema | undefined {
  return typeof ref === "string" ? resolveRef(ref, root) : undefined;
}

function jsonType(value: unknown, allowed: readonly unknown[]): string {
  if (value === null) {
    return "null";
  }
  if (Array.isArray(value)) {
    return "array";
  }
  if (typeof value === "number") {
    return Number.isInteger(value) && allowed.includes("integer")
      ? "integer"
      : "number";
  }
  return typeof value;
}

const pickBranch: Rewrite = ({ schema, value }, root) =>
  (["anyOf", "oneOf"] as const).flatMap((keyword) => {
    const branches = schema[keyword];
    return Array.isArray(branches)
      ? branches.flatMap((branch: unknown, index) =>
          isRecord(branch) && isValid({ value, schema: branch }, root)
            ? [{ edit: `pick ${keyword}[${index}]`, schema: branch, value }]
            : []
        )
      : [];
  });

const inlineRef: Rewrite = ({ schema, value }, root) => {
  const def = resolve(schema["$ref"], root);
  return def === undefined ? [] : [{ edit: "inline $ref", schema: def, value }];
};

const narrowType: Rewrite = ({ schema, value }) => {
  const type = schema["type"];
  return Array.isArray(type)
    ? [
        {
          edit: "narrow type array",
          schema: { ...schema, type: jsonType(value, type) },
          value,
        },
      ]
    : [];
};

const shrinkArray: Rewrite = ({ schema, value }) => {
  const items = schema["items"];
  if (!Array.isArray(value) || value.length === 0) {
    return [];
  }
  return [
    ...(value.length > 1
      ? [{ edit: "keep first item", schema, value: value.slice(0, 1) }]
      : []),
    ...(isRecord(items)
      ? [{ edit: "unwrap array", schema: items, value: value[0] }]
      : []),
  ];
};

const dropProperty: Rewrite = ({ schema, value }) => {
  const properties = schema["properties"];
  if (
    !isRecord(properties) ||
    !isRecord(value) ||
    Object.keys(properties).length < 2
  ) {
    return [];
  }
  const required = Array.isArray(schema["required"]) ? schema["required"] : [];
  return Object.keys(properties).map((key) => ({
    edit: `drop .${key}`,
    schema: {
      ...schema,
      properties: Object.fromEntries(
        Object.entries(properties).filter(([name]) => name !== key)
      ),
      required: required.filter((name) => name !== key),
    },
    value: Object.fromEntries(
      Object.entries(value).filter(([name]) => name !== key)
    ),
  }));
};

const unwrapObject: Rewrite = ({ schema, value }) => {
  const properties = schema["properties"];
  if (!isRecord(properties) || !isRecord(value)) {
    return [];
  }
  return Object.entries(properties).flatMap(([key, child]) =>
    isRecord(child) && key in value
      ? [{ edit: `unwrap .${key}`, schema: child, value: value[key] }]
      : []
  );
};

const shrinkScalar: Rewrite = ({ schema, value }) => {
  if (typeof value === "string" && value.length > 1) {
    const half = [...value].slice(0, Math.ceil([...value].length / 2)).join("");
    return [{ edit: "halve string", schema, value: half }];
  }
  if (typeof value === "number" && value !== 0 && value !== 0.5) {
    return [
      { edit: "zero number", schema, value: Number.isInteger(value) ? 0 : 0.5 },
    ];
  }
  return [];
};

const SIMPLER: readonly Rewrite[] = [
  inlineRef,
  pickBranch,
  narrowType,
  dropProperty,
  unwrapObject,
  shrinkArray,
  shrinkScalar,
];

function childNodes(
  node: Omit<Node, "edit">,
  root: JsonSchema
): readonly Node[] {
  const { schema, value } = node;
  const properties = schema["properties"];
  if (isRecord(properties) && isRecord(value)) {
    return Object.entries(properties).flatMap(([key, child]) =>
      isRecord(child) && key in value
        ? nodeVariants({ schema: child, value: value[key] }, root).map(
            (inner) => ({
              edit: `.${key}: ${inner.edit}`,
              schema: {
                ...schema,
                properties: { ...properties, [key]: inner.schema },
              },
              value: { ...value, [key]: inner.value },
            })
          )
        : []
    );
  }
  const items = schema["items"];
  return isRecord(items) && Array.isArray(value) && value.length === 1
    ? nodeVariants({ schema: items, value: value[0] }, root).map((inner) => ({
        edit: `[0]: ${inner.edit}`,
        schema: { ...schema, items: inner.schema },
        value: [inner.value],
      }))
    : [];
}

function nodeVariants(
  node: Omit<Node, "edit">,
  root: JsonSchema
): readonly Node[] {
  return [
    ...SIMPLER.flatMap((rewrite) => rewrite(node, root)),
    ...childNodes(node, root),
  ];
}

function usedDefs(
  schema: JsonSchema,
  defs: Readonly<Record<string, unknown>>
): JsonSchema {
  const seen = new Set<string>();
  const pending = [JSON.stringify(schema)];
  for (let text = pending.pop(); text !== undefined; text = pending.pop()) {
    for (const [, name = ""] of text.matchAll(/"#\/\$defs\/([^"]+)"/g)) {
      if (!seen.has(name) && isRecord(defs[name])) {
        seen.add(name);
        pending.push(JSON.stringify(defs[name]));
      }
    }
  }
  return seen.size === 0
    ? {}
    : {
        $defs: Object.fromEntries([...seen].map((name) => [name, defs[name]])),
      };
}

function toVariant(node: Node, root: JsonSchema): Variant | undefined {
  const { $defs: _defs, ...schema } = node.schema;
  const defs = isRecord(root["$defs"]) ? root["$defs"] : {};
  const parameters = { ...schema, ...usedDefs(schema, defs) };
  return parameters["type"] === "object" &&
    isRecord(node.value) &&
    !Array.isArray(node.value) &&
    validateJsonSchema(node.value, parameters).length === 0
    ? { edit: node.edit, parameters, args: node.value }
    : undefined;
}

export function simplerVariants({
  parameters,
  args,
}: Pick<Variant, "parameters" | "args">): readonly Variant[] {
  return nodeVariants({ schema: parameters, value: args }, parameters).flatMap(
    (node) => {
      const variant = toVariant(node, parameters);
      return variant === undefined ? [] : [variant];
    }
  );
}

type Grow = (node: Omit<Node, "edit">, name: string) => Node;

const GROW: readonly Grow[] = [
  ({ schema, value }) => ({
    edit: "nullable",
    schema: { anyOf: [schema, { type: "null" }] },
    value,
  }),
  ({ schema, value }) => ({
    edit: "nullable-first",
    schema: { anyOf: [{ type: "null" }, schema] },
    value,
  }),
  ({ schema, value }) => ({
    edit: "wrap in array",
    schema: { type: "array", items: schema },
    value: [value],
  }),
  ({ schema, value }) => ({
    edit: "wrap in object",
    schema: {
      type: "object",
      properties: { value: schema },
      required: ["value"],
      additionalProperties: false,
    },
    value: { value },
  }),
  ({ schema, value }, name) => ({
    edit: "move to $defs",
    schema: { $ref: `#/$defs/${name}`, $defs: { [name]: schema } },
    value,
  }),
];

export function widerVariants({
  parameters,
  args,
}: Pick<Variant, "parameters" | "args">): readonly Variant[] {
  const properties = parameters["properties"];
  if (!isRecord(properties)) {
    return [];
  }
  return Object.entries(properties).flatMap(([key, child]) =>
    isRecord(child) && key in args
      ? GROW.flatMap((grow) => {
          const node = grow(
            { schema: child, value: args[key] },
            `grown_${key}`.replaceAll(/\W/g, "_")
          );
          const { $defs: added, ...schema } = node.schema;
          const defs = {
            ...(isRecord(parameters["$defs"]) ? parameters["$defs"] : {}),
            ...(isRecord(added) ? added : {}),
          };
          const candidate = {
            ...parameters,
            properties: { ...properties, [key]: schema },
            ...(Object.keys(defs).length === 0 ? {} : { $defs: defs }),
          };
          const nextArgs = { ...args, [key]: node.value };
          return validateJsonSchema(nextArgs, candidate).length === 0
            ? [
                {
                  edit: `.${key}: ${node.edit}`,
                  parameters: candidate,
                  args: nextArgs,
                },
              ]
            : [];
        })
      : []
  );
}
