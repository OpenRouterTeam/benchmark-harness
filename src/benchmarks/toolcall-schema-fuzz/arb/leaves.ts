import type { Arbitrary } from "fast-check";
import {
  constant,
  constantFrom,
  integer,
  nat,
  oneof,
  record,
} from "fast-check";

import type { JsonSchema } from "../json-schema";

type Model = Readonly<Record<string, Arbitrary<unknown>>>;

const STRICT_STRING_KEYWORDS = new Set(["pattern", "format"]);
const STRICT_NUMBER_KEYWORDS = new Set([
  "minimum",
  "maximum",
  "exclusiveMinimum",
  "exclusiveMaximum",
  "multipleOf",
]);
const INTEGER_DIVISORS = [2, 3, 4, 5, 10, 100] as const;
const DYADIC_DIVISORS = [0.5, 0.25, 0.125, 0.0625] as const;
const BOUND_SLACK = 100;

const FORMATS: Readonly<Record<string, RegExp>> = {
  "date-time":
    /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/,
  date: /^\d{4}-\d{2}-\d{2}$/,
  time: /^\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/,
  email: /^[^\s@]+@[^\s@]+\.[^\s@]+$/,
  uuid: /^[\da-f]{8}-[\da-f]{4}-[\da-f]{4}-[\da-f]{4}-[\da-f]{12}$/,
  uri: /^[a-z][\d+.a-z-]*:\/\/\S+$/i,
  ipv4: /^(?:(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)\.){3}(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)$/,
  hostname: /^(?:[\da-z](?:[\da-z-]{0,61}[\da-z])?\.)+[a-z]{2,}$/i,
};
const BASE64 = /^(?:[\d+/A-Za-z]{4})*(?:[\d+/A-Za-z]{2}==|[\d+/A-Za-z]{3}=)?$/;

function escapeRegex(text: string): string {
  return text.replaceAll(/[$()*+./?[\\\]^{|}]/g, String.raw`\$&`);
}

function keywords(
  model: Model,
  allowed: ReadonlySet<string> | undefined
): Arbitrary<JsonSchema> {
  const entries = Object.entries(model).filter(
    ([key]) => allowed?.has(key) ?? true
  );
  return record(Object.fromEntries(entries), { requiredKeys: [] });
}

function patterns(value: string): Arbitrary<string> {
  const points = [...value];
  const escaped = escapeRegex(value);
  const first = points[0] ?? "";
  const absent =
    ["Z", "#", "0"].find((candidate) => candidate !== first) ?? "Z";
  return oneof(
    integer({ min: 0, max: Math.min(3, points.length) }).map(
      (length) => `^${escapeRegex(points.slice(0, length).join(""))}`
    ),
    constant(`^${escaped}$`),
    constant(String.raw`^[\s\S]{${points.length}}$`),
    constant(`^(?!${escapeRegex(absent)})`),
    constant(`^(?:${escaped}|${escapeRegex(`${value}_`)})$`),
    ...(/^[\w-]*$/.test(value) ? [constant(String.raw`^[\w-]*$`)] : []),
    ...(/^\d+$/.test(value) ? [constant(String.raw`^\d+$`)] : [])
  );
}

export function stringKeywords(
  value: string,
  strict: boolean
): Arbitrary<JsonSchema> {
  const length = [...value].length;
  const formats = Object.entries(FORMATS).flatMap(([name, regex]) =>
    regex.test(value) ? [name] : []
  );
  const model: Model = {
    minLength: integer({ min: 0, max: length }),
    maxLength: integer({ min: length, max: length + 8 }),
    pattern: patterns(value),
    ...(formats.length > 0 ? { format: constantFrom(...formats) } : {}),
    contentMediaType: constant("text/plain"),
    ...(value.length > 0 && BASE64.test(value)
      ? { contentEncoding: constant("base64") }
      : {}),
  };
  return keywords(model, strict ? STRICT_STRING_KEYWORDS : undefined);
}

function integerModel(value: number): Model {
  const divisors = INTEGER_DIVISORS.filter((divisor) => value % divisor === 0);
  return {
    minimum: nat(BOUND_SLACK).map((slack) => value - slack),
    maximum: nat(BOUND_SLACK).map((slack) => value + slack),
    exclusiveMinimum: nat(BOUND_SLACK).map((slack) => value - slack - 1),
    exclusiveMaximum: nat(BOUND_SLACK).map((slack) => value + slack + 1),
    multipleOf: constantFrom(
      1,
      ...divisors,
      ...(value === 0 ? [] : [Math.abs(value)])
    ),
  };
}

function fractionModel(value: number): Model {
  const slack = Math.abs(value) / 2 + 1;
  const below = value - slack;
  const above = value + slack;
  const divisors = DYADIC_DIVISORS.filter((divisor) =>
    Number.isInteger(value / divisor)
  );
  return {
    ...(Number.isFinite(below)
      ? {
          minimum: constantFrom(value, below),
          exclusiveMinimum: constant(below),
        }
      : {}),
    ...(Number.isFinite(above)
      ? {
          maximum: constantFrom(value, above),
          exclusiveMaximum: constant(above),
        }
      : {}),
    ...(divisors.length > 0 ? { multipleOf: constantFrom(...divisors) } : {}),
  };
}

export function numberKeywords(
  value: number,
  strict: boolean
): Arbitrary<JsonSchema> {
  const model = Number.isInteger(value)
    ? integerModel(value)
    : fractionModel(value);
  return keywords(model, strict ? STRICT_NUMBER_KEYWORDS : undefined);
}
