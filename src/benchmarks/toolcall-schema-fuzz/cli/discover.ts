import { writeFile } from "node:fs/promises";
import { parseArgs } from "node:util";

import { sample } from "fast-check";

import { Either } from "../../../internal/either";
import { parseSchema } from "../../../internal/zod";
import { constructsOf } from "../constructs";
import type { Candidate } from "../discovery/discover";
import { discover, findingSignature } from "../discovery/discover";
import { openRouterProbe } from "../discovery/openrouter-probe";
import { parametersOf, randomDraws } from "../random";
import { openRouterChat, reword } from "../realistic/rewriter";
import {
  RegressionFileSchema,
  TOOLCALL_SCHEMA_FUZZ_REGRESSIONS,
} from "../regressions";

const REGRESSIONS_PATH = new URL("../regressions.json", import.meta.url);

const { values } = parseArgs({
  options: {
    model: { type: "string" },
    targets: { type: "string" },
    reference: { type: "string" },
    count: { type: "string", default: "50" },
    seed: { type: "string", default: "1" },
    repeats: { type: "string", default: "3" },
    budget: { type: "string", default: "2000" },
    write: { type: "boolean", default: false },
    writer: { type: "string", default: "anthropic/claude-sonnet-5.5" },
    checker: { type: "string", default: "openai/gpt-6.1-sol" },
  },
});

const apiKey = process.env["OPENROUTER_API_KEY"];
if (
  values.model === undefined ||
  values.targets === undefined ||
  apiKey === undefined
) {
  process.stderr.write(
    "--model, --targets and OPENROUTER_API_KEY are required\n"
  );
  process.exit(1);
}

const known = new Set(
  TOOLCALL_SCHEMA_FUZZ_REGRESSIONS.entries.map((entry) =>
    findingSignature({
      provider: entry.finding.provider,
      category: entry.finding.category,
      parameters: entry.failing.parameters,
    })
  )
);

const seeds = sample(randomDraws, {
  seed: Number(values.seed),
  numRuns: Number(values.count),
}).map((draw) => ({
  name: draw.name,
  strict: draw.strict,
  parameters: parametersOf(draw.node),
  args: draw.args,
}));

const baseUrl =
  process.env["OPENROUTER_BASE_URL"] ?? "https://openrouter.ai/api/v1";
const rewriter = {
  complete: openRouterChat({ apiKey, baseUrl }),
  writer: values.writer,
  checker: values.checker,
  attempts: 3,
  log: (line: string): void => {
    process.stderr.write(`${line}\n`);
  },
};

async function realize(candidate: Candidate): Promise<Candidate | undefined> {
  const entry = await reword(rewriter, {
    key: constructsOf(candidate.parameters).join(","),
    drawn: {
      name: candidate.name,
      parameters: candidate.parameters,
      args: [candidate.args, candidate.args],
      isDistinct: false,
    },
  });
  if (typeof entry === "string") {
    process.stderr.write(`no realistic rewording: ${entry}\n`);
    return undefined;
  }
  const { name, parameters, args, description, prompts, distractor } = entry;
  return {
    name,
    strict: candidate.strict,
    parameters,
    args: args[0],
    wording: { description, prompts, distractor },
  };
}

const findings = await discover(seeds, {
  realize,
  config: {
    model: values.model,
    targets: values.targets.split(","),
    ...(values.reference === undefined ? {} : { reference: values.reference }),
    repeats: Number(values.repeats),
    budget: Number(values.budget),
    known,
    now: new Date().toISOString(),
  },
  probe: openRouterProbe({
    apiKey,
    model: values.model,
    baseUrl,
    retries: 6,
  }),
});

process.stdout.write(`${JSON.stringify(findings, null, 2)}\n`);

if (values.write && findings.length > 0) {
  const next = parseSchema(RegressionFileSchema, {
    version: TOOLCALL_SCHEMA_FUZZ_REGRESSIONS.version + 1,
    entries: [...TOOLCALL_SCHEMA_FUZZ_REGRESSIONS.entries, ...findings],
  });
  if (Either.isLeft(next)) {
    process.stderr.write(
      `findings do not match the regression schema: ${next.left.message}\n`
    );
    process.exit(1);
  }
  await writeFile(REGRESSIONS_PATH, `${JSON.stringify(next.right, null, 2)}\n`);
}
