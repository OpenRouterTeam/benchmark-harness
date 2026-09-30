import { z, zInt } from "../../internal/zod";
import { DECISION_ACTIONS, PROBABLY_PROGRAM_IDS } from "./programs";

export const PROBABLY_DECISIONS_ID = "probably_decisions" as const;

export const PROBABLY_MODES = ["judgment", "research"] as const;

export type ProbablyMode = (typeof PROBABLY_MODES)[number];

export const DEFAULT_MAX_RESEARCH_STEPS = 16;

export const MAX_DOSSIER_CHARS = 20_000;

export const BUNDLED_DATASET_URL = new URL(
  "__fixtures__/sentinel-sample.jsonl",
  import.meta.url
).href;

const EvidenceSectionSchema = z.record(z.string(), z.unknown());

export const DecisionRecordSchema = z.object({
  id: z.string().min(1),
  domain: z.string().min(1),
  source: z.string().min(1),
  dossier: z.string().min(1),
  evidence: z.record(z.string(), EvidenceSectionSchema),
  gold: z.object({
    outcome: z.enum(DECISION_ACTIONS),
    human_decision: z.enum(["approved", "denied", "reverted"]),
    proposed_kind: z.string().optional(),
    target_count: zInt().nonnegative().optional(),
    facts: z.array(z.string().min(1)).optional(),
  }),
});

export type DecisionRecord = z.infer<typeof DecisionRecordSchema>;

export const DecisionSampleMetaSchema = z.object({
  domain: z.string(),
  source: z.string(),
  lead: z.string(),
  dossier: z.string(),
  evidence: z.record(z.string(), EvidenceSectionSchema),
  humanDecision: z.enum(["approved", "denied", "reverted"]),
  goldFacts: z.array(z.string()),
});

export type DecisionSampleMeta = z.infer<typeof DecisionSampleMetaSchema>;

export const ProbablyDecisionsOptionsSchema = z.object({
  datasetUrl: z.string().url().default(BUNDLED_DATASET_URL),
  mode: z.enum(PROBABLY_MODES).default("judgment"),
  program: z.enum(PROBABLY_PROGRAM_IDS).default("sentinel_case_v1"),
  maxResearchSteps: zInt().positive().default(DEFAULT_MAX_RESEARCH_STEPS),
  judgeModel: z.string().min(1).optional(),
});

export const JudgeTraceSchema = z.object({
  line: zInt(),
  labels: z.array(z.string()),
  probabilities: z.record(z.string(), z.number()),
  chosen: z.string().nullable(),
  threshold: z.number(),
});

export type JudgeTrace = z.infer<typeof JudgeTraceSchema>;

export const ProbablyRunMetaSchema = z.object({
  mode: z.enum(PROBABLY_MODES),
  action: z.string().nullable(),
  judges: z.array(JudgeTraceSchema),
  researchDossier: z.string().nullable(),
  sectionsAvailable: z.array(z.string()).readonly(),
  sectionsRead: z.array(z.string()).readonly(),
  researchSteps: zInt(),
  failure: z.string().nullable(),
});

export type ProbablyRunMeta = z.infer<typeof ProbablyRunMetaSchema>;

export const ScoreDetailSchema = z.object({
  mode: z.enum(PROBABLY_MODES),
  predicted: z.string().nullable(),
  gold: z.string(),
  correct: z.boolean(),
  enactmentAgreement: z.boolean(),
  judgeCount: zInt(),
  meanTopProbability: z.number().nullable(),
  branchAgreement: z.number().nullable(),
  brier: z.number().nullable(),
  evidenceCoverage: z.number().nullable(),
  factRecall: z.number().nullable(),
  failure: z.string().nullable(),
});

export type ScoreDetail = z.infer<typeof ScoreDetailSchema>;
