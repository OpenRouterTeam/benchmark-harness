import type { ZodError, ZodType } from "zod";
import { z } from "zod";

import { Either } from "./either";
export { z };

export type ZodShape<T> = {
  [key in keyof Required<T>]: ZodType<T[key]>;
};

export function parseSchema<Input, Output>(
  schema: ZodType<Output, Input>,
  value: unknown
): Either.Either<Output, ZodError> {
  const ret = schema.safeParse(value);
  if (!ret.success) {
    return Either.left(ret.error);
  }
  return Either.right(ret.data);
}

export function firstZodIssueMessage(error: ZodError): string {
  const issue = error.issues[0];
  if (!issue) {
    return error.message;
  }
  const path = issue.path.length > 0 ? `${issue.path.join(".")}: ` : "";
  return `${path}${issue.message}`;
}

export function zInt(): z.ZodNumber {
  return z.number().int();
}

export function zDefaultedText(
  defaultValue: string
): z.ZodDefault<z.ZodPreprocess<z.ZodDefault<z.ZodString>>> {
  return z
    .preprocess(
      (value) =>
        typeof value === "string" && value.trim() === "" ? undefined : value,
      z.string().default(defaultValue)
    )
    .default(defaultValue);
}
