import { Either } from "../../../internal/either";
import { isRecord } from "../../../internal/guards";
import { getFieldCoalesced } from "./field-access";

export const FROZEN_TODAY_STR = "11/14/2025";

export function getTodayStr(): string {
  return FROZEN_TODAY_STR;
}

export function parseBalance(val: unknown): number {
  if (typeof val === "number") {
    return Number.isFinite(val) ? val : 0;
  }
  if (typeof val === "string") {
    const parsed = Number.parseFloat(val.replace("$", "").replaceAll(",", ""));
    return Number.isFinite(parsed) ? parsed : 0;
  }
  return 0;
}

export function getAccountBalance(account: Record<string, unknown>): number {
  return parseBalance(
    getFieldCoalesced(account, "current_holdings", "balance")
  );
}

export function parseToolArguments(
  argumentsJson: string
): Record<string, unknown> {
  const parsed = Either.try((): unknown => JSON.parse(argumentsJson));
  return Either.isRight(parsed) && isRecord(parsed.right) ? parsed.right : {};
}
