#!/usr/bin/env bun
import { readFileSync } from "node:fs";
import { basename } from "node:path";

import { asyncBufferFromBytes, readResultRows } from "../../results/parquet";
import type { BenchmarkResultRow } from "../../results/parquet-schema";
import {
  armRunFromResultParts,
  compareArms,
  formatArmComparison,
} from "./compare";

interface CompareCliArgs {
  readonly baseline: readonly string[];
  readonly candidate: readonly string[];
  readonly baselineLabel: string;
  readonly candidateLabel: string;
}

export function parseCompareArgs(argv: readonly string[]): CompareCliArgs {
  const get = (flag: string): string | undefined => {
    const idx = argv.indexOf(flag);
    return idx !== -1 ? argv[idx + 1] : undefined;
  };
  const baseline = get("--baseline");
  const candidate = get("--candidate");
  if (baseline === undefined || candidate === undefined) {
    throw new Error(
      "Usage: compare-cli --baseline <parquet[,parquet...]> --candidate <parquet[,parquet...]>"
    );
  }
  const baselineParts = baseline.split(",");
  const candidateParts = candidate.split(",");
  const firstBaseline = baselineParts[0] ?? baseline;
  const firstCandidate = candidateParts[0] ?? candidate;
  return {
    baseline: baselineParts,
    candidate: candidateParts,
    baselineLabel:
      get("--baseline-label") ?? basename(firstBaseline, ".parquet"),
    candidateLabel:
      get("--candidate-label") ?? basename(firstCandidate, ".parquet"),
  };
}

async function main(): Promise<void> {
  const args = parseCompareArgs(process.argv.slice(2));
  const [baselineParts, candidateParts] = await Promise.all([
    readParts(args.baseline),
    readParts(args.candidate),
  ]);
  const comparison = compareArms(
    armRunFromResultParts(args.baselineLabel, baselineParts),
    armRunFromResultParts(args.candidateLabel, candidateParts)
  );
  process.stdout.write(`${formatArmComparison(comparison)}\n`);
}

function readParts(
  paths: readonly string[]
): Promise<(readonly BenchmarkResultRow[])[]> {
  return Promise.all(
    paths.map((path) =>
      readResultRows(asyncBufferFromBytes(readFileSync(path)))
    )
  );
}

if (import.meta.main) {
  await main();
}
