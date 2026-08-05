import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import type { VerdictValue } from "./schemas";
import { VerdictValue as V } from "./schemas";

export type ParsedVerdict = readonly [
  verdict: VerdictValue,
  justification: string,
];

export function parseSingleVerdict(content: string): ParsedVerdict | null {
  const parsed = Either.try(() => JSON.parse(content));
  return tryFromObject(Either.isRight(parsed) ? parsed.right : undefined);
}

function tryFromObject(obj: unknown): ParsedVerdict | null {
  if (!isRecord(obj)) {
    return null;
  }
  const verdict = obj["verdict"];
  if (verdict !== V.Met && verdict !== V.Unmet) {
    return null;
  }
  const justificationRaw = obj["justification"] ?? "";
  const justification =
    typeof justificationRaw === "string"
      ? justificationRaw
      : String(justificationRaw);
  return [verdict, justification] as const;
}
