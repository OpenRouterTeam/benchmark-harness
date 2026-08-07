import { Either } from "../../internal/either";
import type { ValueOf } from "../../internal/guards";
import { isRecord } from "../../internal/guards";
import { wLog } from "../../internal/log";
import { firstZodIssueMessage, parseSchema, z } from "../../internal/zod";
import type { BenchmarkResultRow } from "../../results/parquet-schema";
import type { AlignmentEvidenceKind } from "./discoverability";
import { ALIGNMENT_EVIDENCE_KINDS } from "./discoverability";

export const VERIFIER_VERDICT_KINDS = ["agent", "platform", "fixture"] as const;
export type VerifierVerdictKind = ValueOf<typeof VERIFIER_VERDICT_KINDS>;

const VerifierVerdictSchema = z.object({
  kind: z.enum(VERIFIER_VERDICT_KINDS),
  detail: z.string(),
});
export type VerifierVerdict = z.infer<typeof VerifierVerdictSchema>;

export function parseVerifierVerdict(
  verdictJson: string
): VerifierVerdict | undefined {
  if (verdictJson.trim() === "") {
    return undefined;
  }
  const parsedJson = Either.try((): unknown => JSON.parse(verdictJson));
  if (Either.isLeft(parsedJson)) {
    wLog(
      "agent-dx: verifier verdict is not valid JSON, classification falls back to log scan"
    );
    return undefined;
  }
  const parsed = parseSchema(VerifierVerdictSchema, parsedJson.right);
  if (Either.isLeft(parsed)) {
    wLog(
      "agent-dx: verifier verdict rejected by schema, classification falls back to log scan",
      {
        issue: firstZodIssueMessage(parsed.left),
      }
    );
    return undefined;
  }
  return parsed.right;
}

export function verdictKindFromMetadata(
  metadataJson: string | null
): VerifierVerdictKind | undefined {
  if (metadataJson === null) {
    return undefined;
  }
  const parsedJson = Either.try((): unknown => JSON.parse(metadataJson));
  if (Either.isLeft(parsedJson)) {
    return undefined;
  }
  const parsed = parsedJson.right;
  if (!isRecord(parsed)) {
    return undefined;
  }
  const kind = parsed["verdictKind"];
  return VERIFIER_VERDICT_KINDS.find((candidate) => candidate === kind);
}

export interface RowQuality {
  readonly quality?: number;
  readonly subcheckScore?: number;
}

export function qualityFromMetadata(metadataJson: string | null): RowQuality {
  if (metadataJson === null) {
    return {};
  }
  const parsedJson = Either.try((): unknown => JSON.parse(metadataJson));
  if (Either.isLeft(parsedJson)) {
    wLog("agent-dx: row metadata is not valid JSON, quality degrades to empty");
    return {};
  }
  const parsed = parsedJson.right;
  if (!isRecord(parsed)) {
    return {};
  }
  const quality = parsed["quality"];
  const passed = parsed["subchecksPassed"];
  const total = parsed["subchecksTotal"];
  return {
    ...(typeof quality === "number" && { quality }),
    ...(typeof passed === "number" &&
      typeof total === "number" &&
      total > 0 && { subcheckScore: passed / total }),
  };
}

export interface RowDiagnostics {
  readonly alignment?: number;
  readonly openrouterChosen?: boolean;
  readonly alignmentEvidence?: readonly AlignmentEvidenceKind[];
}

export function diagnosticsFromMetadata(
  metadataJson: string | null
): RowDiagnostics {
  if (metadataJson === null) {
    return {};
  }
  const parsedJson = Either.try((): unknown => JSON.parse(metadataJson));
  if (Either.isLeft(parsedJson)) {
    wLog(
      "agent-dx: row metadata is not valid JSON, diagnostics degrade to empty"
    );
    return {};
  }
  const parsed = parsedJson.right;
  if (!isRecord(parsed)) {
    return {};
  }
  const alignment = parsed["alignment"];
  const openrouterChosen = parsed["openrouterChosen"];
  const rawAlignmentEvidence = parsed["alignmentEvidence"];
  const alignmentEvidence = Array.isArray(rawAlignmentEvidence)
    ? rawAlignmentEvidence.filter((kind): kind is AlignmentEvidenceKind =>
        ALIGNMENT_EVIDENCE_KINDS.some((candidate) => candidate === kind)
      )
    : undefined;
  return {
    ...(typeof alignment === "number" && { alignment }),
    ...(typeof openrouterChosen === "boolean" && { openrouterChosen }),
    ...(alignmentEvidence !== undefined && { alignmentEvidence }),
  };
}

export interface RunTotals {
  readonly totalCost: number;
  readonly totalTokens: number;
}

export function runTotalsFromResultParts(
  parts: readonly (readonly BenchmarkResultRow[])[]
): RunTotals {
  const firstRows = parts
    .map((part) => part[0])
    .filter((row): row is BenchmarkResultRow => row !== undefined);
  return {
    totalCost: firstRows.reduce((sum, row) => sum + row.total_cost, 0),
    totalTokens: firstRows.reduce((sum, row) => sum + row.total_tokens, 0),
  };
}

export function runTotalsFromResultRows(
  rows: readonly BenchmarkResultRow[]
): RunTotals {
  const totalsByPart = new Map(
    rows.map((row) => [
      `${row.created_at}|${row.total_tokens}|${row.total_cost}`,
      { tokens: row.total_tokens, cost: row.total_cost },
    ])
  );
  const parts = [...totalsByPart.values()];
  return {
    totalCost: parts.reduce((sum, part) => sum + part.cost, 0),
    totalTokens: parts.reduce((sum, part) => sum + part.tokens, 0),
  };
}
