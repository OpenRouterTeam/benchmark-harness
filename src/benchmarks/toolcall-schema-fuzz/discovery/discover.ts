import { constructsOf } from "../constructs";
import type { RegressionEntry } from "../regressions";
import { hash } from "../shape";
import type { CaseWording, ToolCallSchemaFuzzCase } from "../tool-case";
import { buildCase, FuzzConstruct, FuzzScenario } from "../tool-case";
import type { Variant } from "./variants";
import { simplerVariants, widerVariants } from "./variants";

export const PASS = "pass";
export const TRANSPORT = "transport";

export interface ProbeOutcome {
  readonly category: string;
  readonly explanation: string;
}

export type Probe = (
  entry: ToolCallSchemaFuzzCase,
  provider: string
) => Promise<ProbeOutcome>;

export interface Candidate extends Pick<Variant, "parameters" | "args"> {
  readonly name: string;
  readonly strict: boolean;
  readonly wording?: CaseWording;
}

export type Realize = (candidate: Candidate) => Promise<Candidate | undefined>;

export interface DiscoveryConfig {
  readonly model: string;
  readonly targets: readonly string[];
  readonly reference?: string;

  readonly repeats: number;

  readonly budget: number;

  readonly known: ReadonlySet<string>;
  readonly now: string;
}

interface Observed {
  readonly isConfirmed: boolean;
  readonly failures: number;
  readonly runs: number;
  readonly category: string;
  readonly explanation: string;
}

interface Search {
  readonly config: DiscoveryConfig;
  readonly probe: Probe;
  readonly realize: Realize;
  spent: number;
}

function caseOf(candidate: Candidate): ToolCallSchemaFuzzCase {
  return buildCase({
    id: "toolcall_schema_fuzz-discovery",
    index: 0,
    scenario: FuzzScenario.Single,
    name: candidate.name,
    strict: candidate.strict,
    parameters: candidate.parameters,
    args: [candidate.args, candidate.args],
    constructs: constructsOf(candidate.parameters),
    ...(candidate.wording === undefined ? {} : { wording: candidate.wording }),
  });
}

function isFailure(category: string): boolean {
  return category !== PASS && category !== TRANSPORT;
}

async function probeOnce(
  search: Search,
  {
    candidate,
    provider,
  }: { readonly candidate: Candidate; readonly provider: string }
): Promise<ProbeOutcome | undefined> {
  if (search.spent >= search.config.budget) {
    return undefined;
  }
  search.spent += 1;
  return search.probe(caseOf(candidate), provider);
}

async function observe(
  search: Search,
  {
    candidate,
    provider,
  }: { readonly candidate: Candidate; readonly provider: string }
): Promise<Observed> {
  const outcomes: ProbeOutcome[] = [];
  for (let run = 0; run < search.config.repeats; run += 1) {
    const outcome = await probeOnce(search, { candidate, provider });
    if (outcome === undefined) {
      break;
    }
    outcomes.push(outcome);
  }
  const scored = outcomes.filter((outcome) => outcome.category !== TRANSPORT);
  const failed = scored.filter((outcome) => isFailure(outcome.category));
  const [first] = failed;
  const isConfirmed =
    scored.length === search.config.repeats &&
    failed.length === scored.length &&
    failed.every((outcome) => outcome.category === first?.category);
  return {
    isConfirmed,
    failures: failed.length,
    runs: scored.length,
    category: first?.category ?? PASS,
    explanation: first?.explanation ?? "",
  };
}

async function shrink(
  search: Search,
  {
    candidate,
    provider,
    category,
  }: {
    readonly candidate: Candidate;
    readonly provider: string;
    readonly category: string;
  }
): Promise<Candidate> {
  for (const variant of simplerVariants(candidate)) {
    if (search.spent >= search.config.budget) {
      return candidate;
    }
    const next = {
      ...candidate,
      parameters: variant.parameters,
      args: variant.args,
    };
    const observed = await observe(search, { candidate: next, provider });
    if (observed.isConfirmed && observed.category === category) {
      return shrink(search, { candidate: next, provider, category });
    }
  }
  return candidate;
}

async function closestPassing(
  search: Search,
  {
    candidate,
    provider,
  }: { readonly candidate: Candidate; readonly provider: string }
): Promise<Candidate | undefined> {
  for (const variant of simplerVariants(candidate)) {
    const next = {
      ...candidate,
      parameters: variant.parameters,
      args: variant.args,
    };
    const observed = await observe(search, { candidate: next, provider });
    if (observed.runs === search.config.repeats && observed.failures === 0) {
      return next;
    }
    if (search.spent >= search.config.budget) {
      return undefined;
    }
  }
  return undefined;
}

const INCIDENTAL: ReadonlySet<string> = new Set([
  FuzzConstruct.Object,
  FuzzConstruct.OpenObject,
]);

export function findingSignature({
  provider,
  category,
  parameters,
}: {
  readonly provider: string;
  readonly category: string;
  readonly parameters: Candidate["parameters"];
}): string {
  const constructs = constructsOf(parameters).filter(
    (construct) => !INCIDENTAL.has(construct)
  );
  return [provider, category, ...constructs.toSorted()].join("|");
}

function toolOf(candidate: Candidate): RegressionEntry["failing"] {
  return {
    name: candidate.name,
    strict: candidate.strict,
    parameters: candidate.parameters,
    args: candidate.args,
    ...(candidate.wording === undefined ? {} : { wording: candidate.wording }),
  };
}

async function realisticFailure(
  search: Search,
  {
    candidate,
    provider,
    category,
  }: {
    readonly candidate: Candidate;
    readonly provider: string;
    readonly category: string;
  }
): Promise<
  { readonly candidate: Candidate; readonly observed: Observed } | undefined
> {
  const realistic = await search.realize(candidate);
  if (realistic === undefined) {
    return undefined;
  }
  const observed = await observe(search, { candidate: realistic, provider });
  return observed.isConfirmed && observed.category === category
    ? { candidate: realistic, observed }
    : undefined;
}

async function realisticControl(
  search: Search,
  {
    candidate,
    provider,
  }: { readonly candidate: Candidate; readonly provider: string }
): Promise<Candidate | undefined> {
  const control = await closestPassing(search, { candidate, provider });
  const realistic =
    control === undefined ? undefined : await search.realize(control);
  if (realistic === undefined) {
    return undefined;
  }
  const observed = await observe(search, { candidate: realistic, provider });
  return observed.runs === search.config.repeats && observed.failures === 0
    ? realistic
    : undefined;
}

async function investigate(
  search: Search,
  {
    candidate,
    provider,
    seen,
  }: {
    readonly candidate: Candidate;
    readonly provider: string;
    readonly seen: Set<string>;
  }
): Promise<RegressionEntry | undefined> {
  const confirmed = await observe(search, { candidate, provider });
  if (!confirmed.isConfirmed) {
    return undefined;
  }
  const minimal = await shrink(search, {
    candidate,
    provider,
    category: confirmed.category,
  });
  const signature = findingSignature({
    provider,
    category: confirmed.category,
    parameters: minimal.parameters,
  });
  if (seen.has(signature)) {
    return undefined;
  }
  const real = await realisticFailure(search, {
    candidate: minimal,
    provider,
    category: confirmed.category,
  });
  if (real === undefined) {
    return undefined;
  }
  const { observed } = real;
  seen.add(signature);
  const control = await realisticControl(search, {
    candidate: minimal,
    provider,
  });
  const { reference } = search.config;
  const referenceObserved =
    reference === undefined
      ? undefined
      : await observe(search, {
          candidate: real.candidate,
          provider: reference,
        });
  return {
    key: `${provider}-${observed.category}-${hash(signature).toString(36)}`,
    failing: toolOf(real.candidate),
    ...(control === undefined ? {} : { control: toolOf(control) }),
    finding: {
      category: observed.category,
      explanation: observed.explanation,
      model: search.config.model,
      provider,
      observed: { failures: observed.failures, runs: observed.runs },
      ...(reference === undefined || referenceObserved === undefined
        ? {}
        : {
            reference: {
              provider: reference,
              observed: {
                failures: referenceObserved.failures,
                runs: referenceObserved.runs,
              },
            },
          }),
      discoveredAt: search.config.now,
    },
  };
}

export async function discover(
  seeds: readonly Candidate[],
  {
    config,
    probe,
    realize,
  }: {
    readonly config: DiscoveryConfig;
    readonly probe: Probe;
    readonly realize: Realize;
  }
): Promise<readonly RegressionEntry[]> {
  const search: Search = { config, probe, realize, spent: 0 };
  const seen = new Set(config.known);
  const findings: RegressionEntry[] = [];
  const candidates = seeds.flatMap((seed) => [
    seed,
    ...widerVariants(seed).map((variant) => ({
      ...seed,
      parameters: variant.parameters,
      args: variant.args,
    })),
  ]);
  for (const candidate of candidates) {
    for (const provider of config.targets) {
      const first = await probeOnce(search, { candidate, provider });
      if (first === undefined) {
        return findings;
      }
      if (!isFailure(first.category)) {
        continue;
      }
      const finding = await investigate(search, { candidate, provider, seen });
      if (finding !== undefined) {
        findings.push(finding);
      }
    }
  }
  return findings;
}
