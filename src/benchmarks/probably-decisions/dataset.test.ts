import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { provide, runPromise } from "effect/Effect";
import { runCollect } from "effect/Stream";

import { Dataset } from "../../harness/dataset";
import {
  decisionRecordToSample,
  leadOf,
  makeDecisionDatasetLayer,
  parseJsonlLines,
  readDecisionSampleMeta,
} from "./dataset";
import { BUNDLED_DATASET_URL, DecisionRecordSchema } from "./schema";

const FIXTURE_LINES = parseJsonlLines(
  readFileSync(fileURLToPath(BUNDLED_DATASET_URL), "utf8")
);

const FIXTURE_RECORDS = FIXTURE_LINES.map((line): unknown => JSON.parse(line));

const LEAK_PATTERNS: Readonly<Record<string, RegExp>> = {
  email: /[\w.%+-]+@[\w-]+\.[a-z]{2,}/i,
  clerkId: /\b(?:user|org|ent)_[A-Za-z0-9]{6,}/,
  apiKey: /sk-or-[A-Za-z0-9-]+/,
  ipv4: /\b\d{1,3}(?:\.\d{1,3}){3}\b/,
  slack: /slack\.com|\b[CDG]0[A-Z0-9]{8,}\b|\b1[5-9]\d{8}\.\d{6}\b/,
  internalLink: /devinenterprise\.com|internal\.openrouter\.ai\//,
  uuid: /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i,
};

function* walkStrings(
  value: unknown,
  path: string
): Generator<readonly [string, string]> {
  if (typeof value === "string") {
    yield [path, value];
    return;
  }
  if (Array.isArray(value)) {
    for (const [i, item] of value.entries()) {
      yield* walkStrings(item, `${path}[${i}]`);
    }
    return;
  }
  if (value !== null && typeof value === "object") {
    for (const [key, item] of Object.entries(value)) {
      yield* walkStrings(item, `${path}.${key}`);
    }
  }
}

const RECORD = {
  id: "sentinel-0123456789ab",
  domain: "trust_and_safety",
  source: "sentinel_ban_candidates",
  dossier:
    "Twelve accounts on one JA4 minted keys within 60s of signup. Two share a card fingerprint and both saw declines.",
  evidence: {
    signup: { "t0.signup_ja4": "t13d1516h2_abc", "t0.age_days": 1 },
    funding: { "t0.declines_30d": 3 },
    case: { target_count: 12 },
  },
  gold: {
    outcome: "frontier_block",
    human_decision: "approved",
    target_count: 12,
    facts: ["t13d1516h2_abc"],
  },
} as const;

describe("probably-decisions dataset", () => {
  test("bundled fixture parses and every record validates", () => {
    expect(FIXTURE_RECORDS.length).toBeGreaterThan(50);
    for (const record of FIXTURE_RECORDS) {
      expect(DecisionRecordSchema.safeParse(record).success).toBe(true);
    }
  });

  test("bundled fixture covers every human decision and every action", () => {
    const rows = FIXTURE_RECORDS.map((record) =>
      DecisionRecordSchema.parse(record)
    );
    const decisions = new Set(rows.map((row) => row.gold.human_decision));
    const outcomes = new Set(rows.map((row) => row.gold.outcome));
    expect([...decisions].sort()).toEqual(["approved", "denied", "reverted"]);
    expect([...outcomes].sort()).toEqual([
      "account_ban",
      "frontier_block",
      "hold",
      "inference_block",
      "key_revocation",
      "throttle",
    ]);
  });

  test("bundled fixture contains no direct identifiers in any string field", () => {
    const hits = FIXTURE_RECORDS.flatMap((record, index) =>
      [...walkStrings(record, `record[${index}]`)].flatMap(([path, text]) =>
        Object.entries(LEAK_PATTERNS).flatMap(([name, pattern]) =>
          pattern.test(text) ? [`${path}: ${name}`] : []
        )
      )
    );
    expect(hits).toEqual([]);
  });

  test("record ids are stable and unique", () => {
    const ids = FIXTURE_RECORDS.map(
      (record) => DecisionRecordSchema.parse(record).id
    );
    expect(new Set(ids).size).toBe(ids.length);
    for (const id of ids) {
      expect(id).toMatch(/^sentinel-[0-9a-f]{12}$/);
    }
  });

  test("decisionRecordToSample maps the record deterministically", () => {
    const sample = decisionRecordToSample(RECORD);
    expect(sample).toEqual(decisionRecordToSample(RECORD));
    expect(sample.id).toBe("probably_decisions-sentinel-0123456789ab");
    expect(sample.input).toBe(RECORD.dossier);
    expect(sample.target).toEqual({ text: "frontier_block" });
    const meta = readDecisionSampleMeta(sample.metadata);
    expect(meta).toBeDefined();
    expect(meta?.lead).toBe(
      "Twelve accounts on one JA4 minted keys within 60s of signup."
    );
    expect(meta?.humanDecision).toBe("approved");
    expect(meta?.goldFacts).toEqual(["t13d1516h2_abc"]);
    expect(meta?.evidence["signup"]).toEqual(RECORD.evidence.signup);
  });

  test("decisionRecordToSample rejects an unknown outcome", () => {
    expect(() =>
      decisionRecordToSample({
        ...RECORD,
        gold: { ...RECORD.gold, outcome: "shadow_ban" },
      })
    ).toThrow(/failed validation/);
  });

  test("leadOf returns the whole dossier when there is one sentence", () => {
    expect(leadOf("Single sentence without terminal punctuation")).toBe(
      "Single sentence without terminal punctuation"
    );
  });

  test("dataset layer streams the bundled fixture with offsets", async () => {
    const layer = makeDecisionDatasetLayer({ datasetUrl: BUNDLED_DATASET_URL });
    const program = Dataset.pipe(provide(layer));
    const dataset = await runPromise(program);
    const size = await runPromise(dataset.size);
    expect(size).toBe(FIXTURE_LINES.length);
    const page = await runPromise(
      runCollect(dataset.stream({ start: 3, end: 8 }))
    );
    const samples = [...page];
    expect(samples).toHaveLength(5);
    expect(samples[0]?.id).toBe(
      `probably_decisions-${DecisionRecordSchema.parse(FIXTURE_RECORDS[3]).id}`
    );
  });
});
