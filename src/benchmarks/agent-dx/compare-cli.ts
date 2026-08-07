#!/usr/bin/env bun
import { readFileSync } from "node:fs";
import { basename } from "node:path";

import { asyncBufferFromBytes, readResultRows } from "../../results/parquet";
import {
  armRunFromResultRows,
  compareArms,
  formatArmComparison,
} from "./compare";

interface CompareCliArgs {
  readonly baseline: string;
  readonly candidate: string;
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
      "Usage: compare-cli --baseline <parquet> --candidate <parquet>"
    );
  }
  return {
    baseline,
    candidate,
    baselineLabel: get("--baseline-label") ?? basename(baseline, ".parquet"),
    candidateLabel: get("--candidate-label") ?? basename(candidate, ".parquet"),
  };
}

async function main(): Promise<void> {
  const args = parseCompareArgs(process.argv.slice(2));
  const [baselineRows, candidateRows] = await Promise.all([
    readResultRows(asyncBufferFromBytes(readFileSync(args.baseline))),
    readResultRows(asyncBufferFromBytes(readFileSync(args.candidate))),
  ]);
  const comparison = compareArms(
    armRunFromResultRows(args.baselineLabel, baselineRows),
    armRunFromResultRows(args.candidateLabel, candidateRows)
  );
  process.stdout.write(`${formatArmComparison(comparison)}\n`);
}

if (import.meta.main) {
  await main();
}
