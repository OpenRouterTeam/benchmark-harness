import { isRecord } from "../../internal/guards";
import type { JsonSchema } from "./json-schema";
import { PRIMITIVES } from "./shape";
import { FuzzConstruct } from "./tool-case";

const IDENTIFIER = /^[A-Za-z_][\w-]*$/;

const KEYWORD_CONSTRUCTS: readonly (readonly [string, FuzzConstruct])[] = [
  ["enum", FuzzConstruct.Enum],
  ["const", FuzzConstruct.Const],
  ["allOf", FuzzConstruct.AllOf],
  ["$ref", FuzzConstruct.Ref],
  ["minLength", FuzzConstruct.StringBounds],
  ["maxLength", FuzzConstruct.StringBounds],
  ["pattern", FuzzConstruct.Pattern],
  ["format", FuzzConstruct.Format],
  ["minimum", FuzzConstruct.NumberBounds],
  ["maximum", FuzzConstruct.NumberBounds],
  ["exclusiveMinimum", FuzzConstruct.NumberBounds],
  ["exclusiveMaximum", FuzzConstruct.NumberBounds],
  ["multipleOf", FuzzConstruct.MultipleOf],
  ["minItems", FuzzConstruct.ArrayBounds],
  ["maxItems", FuzzConstruct.ArrayBounds],
  ["prefixItems", FuzzConstruct.PrefixItems],
  ["description", FuzzConstruct.Description],
  ["not", FuzzConstruct.Not],
  ["if", FuzzConstruct.Conditional],
  ["contains", FuzzConstruct.Contains],
  ["uniqueItems", FuzzConstruct.UniqueItems],
  ["minProperties", FuzzConstruct.PropertyBounds],
  ["maxProperties", FuzzConstruct.PropertyBounds],
  ["patternProperties", FuzzConstruct.PatternProperties],
  ["propertyNames", FuzzConstruct.PropertyNames],
  ["dependentRequired", FuzzConstruct.DependentRequired],
  ["dependentSchemas", FuzzConstruct.DependentSchemas],
  ["title", FuzzConstruct.Annotation],
  ["default", FuzzConstruct.Annotation],
  ["examples", FuzzConstruct.Annotation],
  ["deprecated", FuzzConstruct.Annotation],
  ["readOnly", FuzzConstruct.Annotation],
  ["writeOnly", FuzzConstruct.Annotation],
  ["$comment", FuzzConstruct.Annotation],
  ["contentEncoding", FuzzConstruct.ContentAnnotation],
  ["contentMediaType", FuzzConstruct.ContentAnnotation],
];
const ROOT_COMBINATORS = [
  "anyOf",
  "oneOf",
  "allOf",
  "$ref",
  "not",
  "if",
] as const;
const ESCAPED_POINTER = /~[01]|%[\dA-Fa-f]{2}/;

function typeConstructs(type: unknown): readonly FuzzConstruct[] {
  if (!Array.isArray(type)) {
    return PRIMITIVES.filter((primitive) => primitive === type);
  }
  const members = PRIMITIVES.filter((primitive) => type.includes(primitive));
  return [
    FuzzConstruct.TypeArray,
    ...(type.includes("object") || type.includes("array")
      ? [FuzzConstruct.CompositeTypeArray]
      : []),
    ...members,
    ...(members.includes("null") ? [FuzzConstruct.Nullable] : []),
  ];
}

function objectConstructs(schema: JsonSchema): readonly FuzzConstruct[] {
  const properties = isRecord(schema["properties"])
    ? Object.keys(schema["properties"])
    : [];
  const required = Array.isArray(schema["required"]) ? schema["required"] : [];
  const additional = schema["additionalProperties"];
  return [
    FuzzConstruct.Object,
    ...(additional === false ? [] : [FuzzConstruct.OpenObject]),
    ...(isRecord(additional) ? [FuzzConstruct.AdditionalPropertiesSchema] : []),
    ...(additional === true ? [FuzzConstruct.BooleanSchema] : []),
    ...(properties.some((key) => !required.includes(key))
      ? [FuzzConstruct.OptionalProperty]
      : []),
    ...(properties.some((key) => !IDENTIFIER.test(key))
      ? [FuzzConstruct.UnusualKey]
      : []),
  ];
}

function cardinalityConstructs(schema: JsonSchema): readonly FuzzConstruct[] {
  const counts = (["anyOf", "oneOf", "allOf"] as const).flatMap((keyword) => {
    const branches = schema[keyword];
    return Array.isArray(branches) ? [{ keyword, count: branches.length }] : [];
  });
  return [
    ...(counts.some(({ keyword, count }) => keyword !== "allOf" && count === 1)
      ? [FuzzConstruct.SingleBranchUnion]
      : []),
    ...(counts.some(({ count }) => count >= 3)
      ? [FuzzConstruct.WideCombinator]
      : []),
  ];
}

function hasType(schema: JsonSchema, type: string): boolean {
  const declared = schema["type"];
  return Array.isArray(declared) ? declared.includes(type) : declared === type;
}

function unionConstructs(schema: JsonSchema): readonly FuzzConstruct[] {
  return (["anyOf", "oneOf"] as const).flatMap((keyword) => {
    const branches = schema[keyword];
    if (!Array.isArray(branches)) {
      return [];
    }
    const nullBranch = branches.some(
      (branch) => isRecord(branch) && branch["type"] === "null"
    );
    return [
      keyword === "anyOf" ? FuzzConstruct.AnyOf : FuzzConstruct.OneOf,
      ...(nullBranch ? [FuzzConstruct.Nullable] : []),
    ];
  });
}

function refConstructs(schema: JsonSchema): readonly FuzzConstruct[] {
  const ref = schema["$ref"];
  if (typeof ref !== "string") {
    return [];
  }
  return [
    ...(Object.keys(schema).length > 1 ? [FuzzConstruct.RefSiblings] : []),
    ...(ESCAPED_POINTER.test(ref) ? [FuzzConstruct.EscapedPointer] : []),
  ];
}

function ownConstructs(schema: JsonSchema): readonly FuzzConstruct[] {
  if (Object.keys(schema).length === 0) {
    return [FuzzConstruct.AnySchema];
  }
  const allOf = schema["allOf"];
  return [
    ...refConstructs(schema),
    ...(Array.isArray(allOf) && allOf.length === 1
      ? [FuzzConstruct.SingleAllOf]
      : []),
    ...(hasType(schema, "object") ? objectConstructs(schema) : []),
    ...(hasType(schema, "array") ? [FuzzConstruct.Array] : []),
    ...(schema["type"] === "array" ? [] : typeConstructs(schema["type"])),
    ...KEYWORD_CONSTRUCTS.flatMap(([keyword, construct]) =>
      keyword in schema ? [construct] : []
    ),
    ...unionConstructs(schema),
    ...cardinalityConstructs(schema),
  ];
}

function childSchemas(schema: JsonSchema): readonly JsonSchema[] {
  const nested = [
    schema["items"],
    schema["additionalProperties"],
    schema["anyOf"],
    schema["oneOf"],
    schema["allOf"],
    schema["prefixItems"],
    schema["not"],
    schema["if"],
    schema["then"],
    schema["else"],
    schema["contains"],
    schema["propertyNames"],
  ].flat();
  const maps = [
    schema["properties"],
    schema["patternProperties"],
    schema["dependentSchemas"],
    schema["$defs"],
    schema["definitions"],
  ]
    .filter(isRecord)
    .flatMap(Object.values);
  return [...nested, ...maps].filter(isRecord);
}

function recursiveConstructs(schema: JsonSchema): readonly FuzzConstruct[] {
  const defs = isRecord(schema["$defs"]) ? Object.entries(schema["$defs"]) : [];
  return defs.some(([name, def]) =>
    JSON.stringify(def).includes(`"#/$defs/${name}"`)
  )
    ? [FuzzConstruct.RecursiveRef]
    : [];
}

function rootConstructs(schema: JsonSchema): readonly FuzzConstruct[] {
  return [
    ...(ROOT_COMBINATORS.some((keyword) => keyword in schema)
      ? [FuzzConstruct.RootCombinator]
      : []),
    ...(isRecord(schema["definitions"]) ? [FuzzConstruct.Definitions] : []),
  ];
}

function walk(schema: JsonSchema): readonly FuzzConstruct[] {
  return [...ownConstructs(schema), ...childSchemas(schema).flatMap(walk)];
}

export function constructsOf(schema: JsonSchema): readonly FuzzConstruct[] {
  return [
    ...new Set([
      ...walk(schema),
      ...recursiveConstructs(schema),
      ...rootConstructs(schema),
    ]),
  ];
}
