import { ScoreValue } from "../../harness/core";
import { Either } from "../../internal/either";
import type { ValueOf } from "../../internal/guards";
import { isRecord } from "../../internal/guards";
import type { BenchmarkResultRow } from "../../results/parquet-schema";

export const Stability = {
  PassedEveryRun: "passed_every_run",
  FailedEveryRun: "failed_every_run",
  FailedSomeRuns: "failed_some_runs",
  TransportOnly: "transport_only",
} as const;
export type Stability = ValueOf<typeof Stability>;

const TRANSPORT = "transport";
const PASS = "pass";

export type StabilityRow = Pick<
  BenchmarkResultRow,
  "sample_id" | "epoch" | "score_value" | "explanation" | "metadata"
>;

export interface CaseStability {
  readonly id: string;
  readonly scenario: string | undefined;
  readonly constructs: readonly string[];
  readonly control: string | undefined;

  readonly runs: number;
  readonly passes: number;
  readonly transport: number;
  readonly stability: Stability;

  readonly failures: Readonly<Record<string, number>>;

  readonly examples: Readonly<Record<string, string>>;

  readonly controlStability: Stability | undefined;
}

export interface GroupStability {
  readonly key: string;
  readonly cases: number;
  readonly runs: number;
  readonly passes: number;
  readonly flaky: number;
}

export interface StabilityReport {
  readonly cases: readonly CaseStability[];
  readonly constructs: readonly GroupStability[];
  readonly scenarios: readonly GroupStability[];
  readonly totals: GroupStability;
}

interface SampleMetadata {
  readonly scenario: string | undefined;
  readonly constructs: readonly string[];
  readonly control: string | undefined;
}

function outcomeOf(row: StabilityRow): string {
  if (row.score_value === ScoreValue.Correct) {
    return PASS;
  }
  const explanation = row.explanation ?? "";
  if (
    row.score_value === ScoreValue.Skipped ||
    explanation.startsWith("Model error")
  ) {
    return TRANSPORT;
  }
  return explanation.split(":", 1)[0] ?? explanation;
}

function stringField(
  record: Readonly<Record<string, unknown>>,
  key: string
): string | undefined {
  const value = record[key];
  return typeof value === "string" ? value : undefined;
}

function metadataOf(row: StabilityRow): SampleMetadata {
  const parsed = Either.try((): unknown => JSON.parse(row.metadata ?? "{}"));
  const record =
    Either.isRight(parsed) && isRecord(parsed.right) ? parsed.right : {};
  const constructs = record["constructs"];
  return {
    scenario: stringField(record, "scenario"),
    constructs: Array.isArray(constructs)
      ? constructs.filter((item) => typeof item === "string")
      : [],
    control: stringField(record, "control"),
  };
}

function stabilityOf({
  runs,
  passes,
}: {
  readonly runs: number;
  readonly passes: number;
}): Stability {
  if (runs === 0) {
    return Stability.TransportOnly;
  }
  if (passes === runs) {
    return Stability.PassedEveryRun;
  }
  return passes === 0 ? Stability.FailedEveryRun : Stability.FailedSomeRuns;
}

function summarizeCase(
  rows: readonly StabilityRow[]
): Omit<CaseStability, "controlStability"> {
  const [first] = rows;
  const metadata =
    first === undefined
      ? metadataOf({
          sample_id: "",
          epoch: 0,
          score_value: "",
          explanation: null,
          metadata: null,
        })
      : metadataOf(first);
  const outcomes = rows.map((row) => ({
    outcome: outcomeOf(row),
    explanation: row.explanation ?? "",
  }));
  const scored = outcomes.filter(({ outcome }) => outcome !== TRANSPORT);
  const failed = scored.filter(({ outcome }) => outcome !== PASS);
  const passes = scored.length - failed.length;
  const failures: Record<string, number> = {};
  const examples: Record<string, string> = {};
  for (const { outcome, explanation } of failed) {
    failures[outcome] = (failures[outcome] ?? 0) + 1;
    examples[outcome] ??= explanation;
  }
  return {
    id: first?.sample_id ?? "",
    ...metadata,
    runs: scored.length,
    passes,
    transport: outcomes.length - scored.length,
    stability: stabilityOf({ runs: scored.length, passes }),
    failures,
    examples,
  };
}

function groupOf(key: string, cases: readonly CaseStability[]): GroupStability {
  return {
    key,
    cases: cases.length,
    runs: cases.reduce((sum, entry) => sum + entry.runs, 0),
    passes: cases.reduce((sum, entry) => sum + entry.passes, 0),
    flaky: cases.filter((entry) => entry.stability === Stability.FailedSomeRuns)
      .length,
  };
}

function groupsBy(
  cases: readonly CaseStability[],
  keysOf: (entry: CaseStability) => readonly string[]
): readonly GroupStability[] {
  const grouped = Map.groupBy(
    cases.flatMap((entry) => keysOf(entry).map((key) => ({ key, entry }))),
    ({ key }) => key
  );
  return [...grouped]
    .map(([key, members]) =>
      groupOf(
        key,
        members.map(({ entry }) => entry)
      )
    )
    .toSorted((a, b) => a.key.localeCompare(b.key));
}

export function stabilityReport(
  rows: readonly StabilityRow[]
): StabilityReport {
  const summaries = [...Map.groupBy(rows, (row) => row.sample_id).values()].map(
    summarizeCase
  );
  const byId = new Map(summaries.map((entry) => [entry.id, entry.stability]));
  const cases = summaries.map((entry) => ({
    ...entry,
    controlStability:
      entry.control === undefined ? undefined : byId.get(entry.control),
  }));
  return {
    cases,
    constructs: groupsBy(cases, (entry) => entry.constructs),
    scenarios: groupsBy(cases, (entry) =>
      entry.scenario === undefined ? [] : [entry.scenario]
    ),
    totals: groupOf("all", cases),
  };
}
