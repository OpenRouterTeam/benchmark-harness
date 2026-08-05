import { isDefinedAndNotNull, isRecord } from "../../../internal/guards";

export function getFieldAsString(
  record: Record<string, unknown>,
  field: string
): string | undefined {
  const value = record[field];
  return typeof value === "string" ? value : undefined;
}

export function getFieldAsNumber(
  record: Record<string, unknown>,
  field: string
): number | undefined {
  const value = record[field];
  return typeof value === "number" ? value : undefined;
}

export function getFieldAsBoolean(
  record: Record<string, unknown>,
  field: string
): boolean {
  const value = record[field];
  return typeof value === "boolean" ? value : false;
}

export function getFieldAsRecord(
  record: Record<string, unknown>,
  field: string
): Record<string, unknown> | undefined {
  const value = record[field];
  return isRecord(value) ? value : undefined;
}

export function getFieldAsArray(
  record: Record<string, unknown>,
  field: string
): unknown[] | undefined {
  const value = record[field];
  return Array.isArray(value) ? value : undefined;
}

export function getFieldCoalesced(
  record: Record<string, unknown>,
  ...fields: string[]
): unknown {
  for (const field of fields) {
    const value = record[field];
    if (isDefinedAndNotNull(value)) {
      return value;
    }
  }
  return undefined;
}
