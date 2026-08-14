#!/usr/bin/env bun
import { readFileSync } from "node:fs";

import { asyncBufferFromBytes, readResultRows } from "../../results/parquet";
import { formatTrialTrace, tracesFromResultRows } from "./trace";

interface TraceCliArgs {
  readonly results: string;
  readonly task: string | undefined;
  readonly failedOnly: boolean;
}

export function parseTraceArgs(argv: readonly string[]): TraceCliArgs {
  const get = (flag: string): string | undefined => {
    const idx = argv.indexOf(flag);
    return idx !== -1 ? argv[idx + 1] : undefined;
  };
  const results = get("--results");
  if (results === undefined) {
    throw new Error(
      "Usage: trace-cli --results <parquet> [--task <sample_id>] [--failed-only]"
    );
  }
  return {
    results,
    task: get("--task"),
    failedOnly: argv.includes("--failed-only"),
  };
}

async function main(): Promise<void> {
  const args = parseTraceArgs(process.argv.slice(2));
  const rows = await readResultRows(
    asyncBufferFromBytes(readFileSync(args.results))
  );
  const traces = tracesFromResultRows(rows)
    .filter((trace) => args.task === undefined || trace.taskId === args.task)
    .filter((trace) => !args.failedOnly || !trace.passed);
  process.stdout.write(
    `${traces.map((trace) => formatTrialTrace(trace)).join("\n\n")}\n`
  );
}

if (import.meta.main) {
  await main();
}
