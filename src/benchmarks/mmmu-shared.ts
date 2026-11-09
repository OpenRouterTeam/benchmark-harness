import * as Result from "effect/Result";

import { parseSchema, z } from "../internal/zod";

export const MMMU_SYSTEM_MESSAGE = "You are a helpful assistant.";

export const MMMU_MAX_TOKENS = 1024;

const OptionsSchema = z.array(z.string());

export function parseOptions(optionsRaw: unknown): readonly string[] {
  if (Array.isArray(optionsRaw)) {
    const parsed = parseSchema(OptionsSchema, optionsRaw);
    return Result.isFailure(parsed) ? [] : parsed.success;
  }
  if (typeof optionsRaw !== "string") {
    return [];
  }
  const trimmed = optionsRaw.trim();
  if (!trimmed.startsWith("[") || !trimmed.endsWith("]")) {
    return [];
  }
  const jsonResult = Result.try(() =>
    parseSchema(OptionsSchema, JSON.parse(trimmed))
  );
  if (Result.isSuccess(jsonResult) && Result.isSuccess(jsonResult.success)) {
    return jsonResult.success.success;
  }
  const jsonified = trimmed.replaceAll("'", '"');
  const jsonifiedResult = Result.try(() =>
    parseSchema(OptionsSchema, JSON.parse(jsonified))
  );
  if (
    Result.isSuccess(jsonifiedResult) &&
    Result.isSuccess(jsonifiedResult.success)
  ) {
    return jsonifiedResult.success.success;
  }
  return [];
}
