import { Either } from "../internal/either";
import { parseSchema, z } from "../internal/zod";

export const MMMU_SYSTEM_MESSAGE = "You are a helpful assistant.";

export const MMMU_MAX_TOKENS = 1024;

const OptionsSchema = z.array(z.string());

export function parseOptions(optionsRaw: unknown): readonly string[] {
  if (Array.isArray(optionsRaw)) {
    const parsed = parseSchema(OptionsSchema, optionsRaw);
    return Either.isLeft(parsed) ? [] : parsed.right;
  }
  if (typeof optionsRaw !== "string") {
    return [];
  }
  const trimmed = optionsRaw.trim();
  if (!trimmed.startsWith("[") || !trimmed.endsWith("]")) {
    return [];
  }
  const jsonResult = Either.try(() =>
    parseSchema(OptionsSchema, JSON.parse(trimmed))
  );
  if (Either.isRight(jsonResult) && Either.isRight(jsonResult.right)) {
    return jsonResult.right.right;
  }
  const jsonified = trimmed.replaceAll("'", '"');
  const jsonifiedResult = Either.try(() =>
    parseSchema(OptionsSchema, JSON.parse(jsonified))
  );
  if (
    Either.isRight(jsonifiedResult) &&
    Either.isRight(jsonifiedResult.right)
  ) {
    return jsonifiedResult.right.right;
  }
  return [];
}
