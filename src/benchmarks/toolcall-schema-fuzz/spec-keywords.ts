import { SchemaPosition } from "./schema-positions";

type Coverage =
  | { readonly generated: readonly SchemaPosition[] }
  | { readonly skipped: string };

type SpecKeyword =
  | "$schema"
  | "$id"
  | "$ref"
  | "$anchor"
  | "$dynamicRef"
  | "$dynamicAnchor"
  | "$vocabulary"
  | "$comment"
  | "$defs"
  | "prefixItems"
  | "items"
  | "contains"
  | "additionalProperties"
  | "properties"
  | "patternProperties"
  | "dependentSchemas"
  | "propertyNames"
  | "if"
  | "then"
  | "else"
  | "allOf"
  | "anyOf"
  | "oneOf"
  | "not"
  | "unevaluatedItems"
  | "unevaluatedProperties"
  | "type"
  | "const"
  | "enum"
  | "multipleOf"
  | "maximum"
  | "exclusiveMaximum"
  | "minimum"
  | "exclusiveMinimum"
  | "maxLength"
  | "minLength"
  | "pattern"
  | "maxItems"
  | "minItems"
  | "uniqueItems"
  | "maxContains"
  | "minContains"
  | "maxProperties"
  | "minProperties"
  | "required"
  | "dependentRequired"
  | "title"
  | "description"
  | "default"
  | "deprecated"
  | "readOnly"
  | "writeOnly"
  | "examples"
  | "format"
  | "contentEncoding"
  | "contentMediaType"
  | "contentSchema"
  | "definitions"
  | "dependencies"
  | "nullable";

const { Root, Property, Item, Branch, Definition, RefSibling } = SchemaPosition;
const NESTED = [Property, Item, Branch, Definition] as const;
const ANYWHERE = [Root, ...NESTED] as const;
const BESIDE_REF = [...ANYWHERE, RefSibling] as const;

const UNSUPPORTED_BY_VALIDATOR =
  "needs annotation collection or URI resolution, which the scoring validator (json-schema.ts) does not implement";

const SPEC_KEYWORDS = {
  $schema: { generated: [Root] },
  $id: {
    skipped: `changes the base URI for $ref; ${UNSUPPORTED_BY_VALIDATOR}`,
  },
  $ref: { generated: ANYWHERE },
  $anchor: { skipped: `plain-name fragments; ${UNSUPPORTED_BY_VALIDATOR}` },
  $dynamicRef: { skipped: `dynamic scope; ${UNSUPPORTED_BY_VALIDATOR}` },
  $dynamicAnchor: { skipped: `dynamic scope; ${UNSUPPORTED_BY_VALIDATOR}` },
  $vocabulary: {
    skipped: "only meaningful in a meta-schema, never in tool parameters",
  },
  $comment: { generated: BESIDE_REF },
  $defs: { generated: [Root] },
  prefixItems: { generated: NESTED },
  items: { generated: NESTED },
  contains: { generated: NESTED },
  additionalProperties: { generated: ANYWHERE },
  properties: { generated: ANYWHERE },
  patternProperties: { generated: ANYWHERE },
  dependentSchemas: { generated: ANYWHERE },
  propertyNames: { generated: ANYWHERE },
  if: { generated: ANYWHERE },
  // oxlint-disable-next-line unicorn/no-thenable -- `then` is the JSON Schema conditional keyword, not a thenable
  then: { generated: ANYWHERE },
  else: { generated: ANYWHERE },
  allOf: { generated: ANYWHERE },
  anyOf: { generated: ANYWHERE },
  oneOf: { generated: ANYWHERE },
  not: { generated: ANYWHERE },
  unevaluatedItems: {
    skipped: `evaluates sibling annotations; ${UNSUPPORTED_BY_VALIDATOR}`,
  },
  unevaluatedProperties: {
    skipped: `evaluates sibling annotations; ${UNSUPPORTED_BY_VALIDATOR}`,
  },
  type: { generated: BESIDE_REF },
  const: { generated: NESTED },
  enum: { generated: NESTED },
  multipleOf: { generated: NESTED },
  maximum: { generated: NESTED },
  exclusiveMaximum: { generated: NESTED },
  minimum: { generated: NESTED },
  exclusiveMinimum: { generated: NESTED },
  maxLength: { generated: NESTED },
  minLength: { generated: NESTED },
  pattern: { generated: NESTED },
  maxItems: { generated: NESTED },
  minItems: { generated: NESTED },
  uniqueItems: { generated: NESTED },
  maxContains: { generated: NESTED },
  minContains: { generated: NESTED },
  maxProperties: { generated: ANYWHERE },
  minProperties: { generated: ANYWHERE },
  required: { generated: ANYWHERE },
  dependentRequired: { generated: ANYWHERE },
  title: { generated: BESIDE_REF },
  description: { generated: BESIDE_REF },
  default: { generated: BESIDE_REF },
  deprecated: { generated: BESIDE_REF },
  readOnly: { generated: ANYWHERE },
  writeOnly: { generated: ANYWHERE },
  examples: { generated: BESIDE_REF },
  format: { generated: NESTED },
  contentEncoding: { generated: NESTED },
  contentMediaType: { generated: NESTED },
  contentSchema: {
    skipped:
      "describes decoded content; no tool-call path decodes strings before parsing",
  },
  definitions: { generated: [Root] },
  dependencies: {
    skipped:
      "draft-07 form of dependentRequired/dependentSchemas, which are generated; 2020-12 ignores it, so validity is ambiguous",
  },
  nullable: {
    skipped:
      "OpenAPI 3.0 only; 2020-12 ignores it, so whether null is valid is ambiguous. Nullability is generated as anyOf [X, null] and type arrays",
  },
} as const satisfies Record<SpecKeyword, Coverage>;

export const REQUIRED_KEYWORD_POSITIONS: readonly string[] = Object.entries(
  SPEC_KEYWORDS
).flatMap(([keyword, coverage]) =>
  "generated" in coverage
    ? coverage.generated.map((position) => `${keyword}@${position}`)
    : []
);
