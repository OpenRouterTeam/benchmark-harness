/**
 * EvalManifestV1 — the unified description of any eval on the platform:
 * first-party benchmarks, catalog entries, and user-provided evals are all
 * the same object at different binding completeness.
 *
 * Every eval decomposes into dataset → solver → scorer → reducer → reports.
 * The manifest carries the declarative fields all evals share, plus one
 * *binding* per implementation slot:
 *
 * - `builtin` — a named TypeScript implementation compiled into this harness
 *   (how the first-party benchmarks bind; options validated per-builtin).
 * - `spec`    — fully declarative, no code (custom_eval's surface; rung-1
 *   user evals).
 * - `capsule` — a digest-pinned container speaking the capsule I/O contract
 *   (upstream harness adapters and rung-3 user containers).
 *
 * Identity is structural: {@link manifestIdentityDigest} hashes the fields
 * that make scores comparable. Prose (`provenance.comparabilityNotes`)
 * documents *why* — it is never itself the identity.
 *
 * Workflow-safe: schema-only module, no heavy imports. Standalone-clean:
 * vendored zod only. See docs/plans/2026-07-28-002-feat-unified-eval-
 * manifest-architecture.md.
 */
import { createHash } from "node:crypto";

import { z } from "../internal/zod";
import { EvalDatasetSchema, EvalScorerSchema } from "./custom-eval/spec";

export const EVAL_MANIFEST_API_VERSION = "openrouter.ai/eval-manifest/v1";

//#region Model roles

/**
 * Roles are first-class because they are the gateway's unit of budget
 * scoping (one capability token per role) and the per-role spend breakdown
 * in results. `candidate` is implicit in every run; benchmarks that simulate
 * a user (tau) or grade with a model (swe-atlas, judged specs) declare it.
 */
export const ModelRoleSchema = z.object({
  role: z.enum(["candidate", "judge", "user-simulator", "extractor"]),
  /** Default model slug for non-candidate roles; the run config may override. */
  defaultModel: z.string().optional(),
  /** Whether a run may substitute another model for this role. */
  overridable: z.boolean().default(true),
});
export type ModelRole = z.infer<typeof ModelRoleSchema>;

//#endregion

//#region Bindings

const CapsuleResourcesSchema = z.object({
  cpu: z.number().positive().optional(),
  memoryGb: z.number().positive().optional(),
  timeoutMs: z.number().int().positive().optional(),
});

/** Digest-pinned image reference: name@sha256:<64 LOWERCASE hex> — registries
 * treat digests as lowercase-only, and case variants would fork the identity
 * digest for the same image. */
export const PINNED_IMAGE_RE = /^[\w.\-/:]+@sha256:[a-f0-9]{64}$/;

const CapsuleBindingSchema = z.object({
  kind: z.literal("capsule"),
  /** Must be digest-pinned; floating tags are rejected at parse time. */
  image: z
    .string()
    .regex(
      PINNED_IMAGE_RE,
      "capsule image must be digest-pinned (name@sha256:<64-hex>)"
    ),
  command: z.array(z.string()).min(1),
  resources: CapsuleResourcesSchema.optional(),
  /** Capsules never get open egress; the gateway is the only model path. */
  network: z.enum(["none", "gateway-only"]).default("none"),
});

const BuiltinBindingSchema = z.object({
  kind: z.literal("builtin"),
  /** Name of a TS implementation registered in this harness build. */
  ref: z.string().min(1),
  /** Validated against the builtin's own options schema at resolve time. */
  options: z.record(z.string(), z.unknown()).optional(),
});

export const SolverBindingSchema = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("spec"),
    systemPrompt: z.string().optional(),
    /** `{input}` is replaced with the case input; omitted → raw input. */
    promptTemplate: z.string().optional(),
  }),
  BuiltinBindingSchema,
  CapsuleBindingSchema,
]);
export type SolverBinding = z.infer<typeof SolverBindingSchema>;

export const ScorerBindingSchema = z.discriminatedUnion("kind", [
  /** Deterministic declarative scorers (custom_eval's set). */
  z.object({ kind: z.literal("spec"), method: EvalScorerSchema }),
  /**
   * LLM-judged scoring with a pinned judge. Requires a `judge` role in
   * `models[]`; results are labeled judge-dependent, never deterministic.
   */
  z.object({
    kind: z.literal("spec-judge"),
    /** Content-addressed ref to the pinned judge prompt. */
    promptRef: z.string().min(1),
    rubric: z.unknown().optional(),
  }),
  BuiltinBindingSchema,
  CapsuleBindingSchema,
]);
export type ScorerBinding = z.infer<typeof ScorerBindingSchema>;

export const DatasetBindingSchema = z.discriminatedUnion("kind", [
  ...EvalDatasetSchema.options,
  /** Fixture/composite datasets owned by a builtin (tau domains, draco panels). */
  z.object({ kind: z.literal("builtin"), ref: z.string().min(1) }),
]);
export type DatasetBinding = z.infer<typeof DatasetBindingSchema>;

//#endregion

//#region Declarative shared fields

export const ManifestSamplingSchema = z.object({
  temperature: z.number().optional(),
  maxTokens: z.number().int().positive().optional(),
  reasoningEffort: z.string().optional(),
});

export const EpochPolicySchema = z.object({
  default: z.number().int().positive(),
  reducer: z.enum(["mean", "best", "pass@k"]),
  /** Required when reducer is pass@k. */
  k: z.number().int().positive().optional(),
});

/**
 * Identity-bearing media configuration: these settings change scores, so
 * they are part of the comparability digest (image detail for MMMU, video
 * frame policy for Video-MME, context buckets for long-context suites).
 */
export const MediaRecipeSchema = z.object({
  imageDetail: z.enum(["low", "high", "auto"]).optional(),
  video: z
    .object({
      frames: z.number().int().positive().optional(),
      fps: z.number().positive().optional(),
      subtitles: z.enum(["off", "on"]).optional(),
    })
    .optional(),
  context: z
    .object({
      requestedTokens: z.number().int().positive(),
      bucket: z.string(),
    })
    .optional(),
});

export const ExecutionPolicySchema = z.object({
  /**
   * trusted-local: operator-trusted code in-process on the operator's own
   * machine. rootless: hardened containers — the only tier for hosted user
   * code. The tier is a property of trust, not of the code.
   */
  tier: z.enum(["trusted-local", "rootless"]),
  drivers: z.array(z.enum(["process", "docker", "compose"])).min(1),
});

export const ManifestReportSchema = z.object({
  key: z.string(),
  label: z.string(),
  type: z.enum([
    "scalar",
    "table",
    "distribution",
    "confusion",
    "reliability",
    "artifact",
  ]),
  metric: z.string().optional(),
  groupBy: z.array(z.string()).optional(),
  goal: z.enum(["higher", "lower"]).optional(),
});

/** Catalog/coverage metadata. Prose belongs here — and only here. */
export const ProvenanceSchema = z.object({
  canonicalName: z.string().optional(),
  authoritativeRuntime: z.string().optional(),
  sourceUrl: z.string().optional(),
  license: z.string().optional(),
  comparabilityNotes: z.string().optional(),
  section: z.string().optional(),
  supportStatus: z.enum(["ready", "adapter-declared", "pending-smoke"]),
});

//#endregion

//#region Manifest

export const EvalManifestSchema = z.object({
  apiVersion: z.literal(EVAL_MANIFEST_API_VERSION),
  /** 'gpqa_diamond' | 'org_abc/my-regression-suite' */
  id: z.string().min(1).max(200),
  name: z.string().min(1).max(200),
  /** Manifest revision; part of the comparability identity. */
  version: z.string().min(1),
  /** Comparability series key; scores merge only within one protocolId. */
  protocolId: z.string().min(1),
  dataset: DatasetBindingSchema,
  models: z.array(ModelRoleSchema).min(1),
  solver: SolverBindingSchema,
  scorer: ScorerBindingSchema,
  sampling: ManifestSamplingSchema.optional(),
  epochs: EpochPolicySchema,
  mediaRecipe: MediaRecipeSchema.optional(),
  execution: ExecutionPolicySchema,
  reports: z.array(ManifestReportSchema).min(1),
  provenance: ProvenanceSchema.optional(),
});
export type EvalManifestV1 = z.infer<typeof EvalManifestSchema>;

//#endregion

//#region Cross-field validation

export interface ManifestIssue {
  readonly code: string;
  readonly message: string;
}

/**
 * Structural rules zod's field-level schemas cannot express. Fail-closed:
 * a manifest with issues must not resolve to a runnable plan.
 */
export function validateManifest(
  manifest: EvalManifestV1
): readonly ManifestIssue[] {
  const issues: ManifestIssue[] = [];
  const roles = new Set(manifest.models.map((m) => m.role));

  if (!roles.has("candidate")) {
    issues.push({
      code: "missing-candidate",
      message: "models[] must declare a candidate role",
    });
  }
  if (manifest.models.length !== roles.size) {
    issues.push({
      code: "duplicate-role",
      message: "each role may be declared at most once",
    });
  }
  if (manifest.scorer.kind === "spec-judge" && !roles.has("judge")) {
    issues.push({
      code: "judge-role-missing",
      message: "spec-judge scorer requires a judge role in models[]",
    });
  }
  if (manifest.epochs.reducer === "pass@k" && manifest.epochs.k === undefined) {
    issues.push({
      code: "k-missing",
      message: "pass@k reducer requires epochs.k",
    });
  }
  if (
    manifest.epochs.k !== undefined &&
    manifest.epochs.k > manifest.epochs.default
  ) {
    issues.push({
      code: "k-exceeds-epochs",
      message: "epochs.k cannot exceed epochs.default",
    });
  }

  const hasCapsule =
    manifest.solver.kind === "capsule" || manifest.scorer.kind === "capsule";
  if (hasCapsule && manifest.execution.tier !== "rootless") {
    issues.push({
      code: "capsule-needs-rootless",
      message: "capsule bindings require the rootless execution tier",
    });
  }
  if (
    hasCapsule &&
    !manifest.execution.drivers.some((d) => d === "docker" || d === "compose")
  ) {
    issues.push({
      code: "capsule-needs-container-driver",
      message: "capsule bindings require a docker or compose driver",
    });
  }
  return issues;
}

//#endregion

//#region Identity

function stableJson(value: unknown): string {
  if (value === null || typeof value !== "object") {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableJson(item)).join(",")}]`;
  }
  // oxlint-disable-next-line openrouter/no-unnecessary-typecast -- narrowed to a non-null, non-array object above
  const record = value as Record<string, unknown>;
  const body = Object.keys(record)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${stableJson(record[key])}`)
    .join(",");
  return `{${body}}`;
}

/**
 * The comparability identity: hash of every field that changes what a score
 * means. Scores from two runs may aggregate only when their digests match.
 * `name`, `reports`, and `provenance` are display/metadata and excluded.
 */
export function manifestIdentityDigest(manifest: EvalManifestV1): string {
  const identity = {
    protocolId: manifest.protocolId,
    version: manifest.version,
    dataset: manifest.dataset,
    /* Roles are a semantically unordered set (uniqueness is enforced by
     * validateManifest); sort so declaration order cannot split a series. */
    models: [...manifest.models].sort((a, b) => a.role.localeCompare(b.role)),
    solver: manifest.solver,
    scorer: manifest.scorer,
    sampling: manifest.sampling ?? null,
    epochs: manifest.epochs,
    mediaRecipe: manifest.mediaRecipe ?? null,
  };
  return createHash("sha256")
    .update(stableJson(identity))
    .digest("hex")
    .slice(0, 32);
}

/** Whether scoring is reproducible from stored trajectories without a model. */
export function isDeterministicScoring(manifest: EvalManifestV1): boolean {
  return manifest.scorer.kind === "spec";
}

//#endregion
