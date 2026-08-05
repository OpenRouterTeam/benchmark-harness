import {
  COST_TIERS,
  IMAGE_DETAIL_VALUES,
  REASONING_EFFORTS,
} from "../harness/constants";
import { ProviderSort } from "../internal/enums";
import type { ValueOf } from "../internal/guards";
import { z, zDefaultedText, zInt } from "../internal/zod";
import {
  TAU3_BENCH_BANKING_META,
  TAU_BENCH_AIRLINE_META,
} from "./benchmark-meta";
import { EvalSpecSchema } from "./custom-eval/spec";
import { DEFAULT_STEP_LIMIT as DEEP_SWE_DEFAULT_STEP_LIMIT } from "./deep-swe/schema";
import { DracoPanelConfigSchema } from "./draco/schemas";
import { SearchLaneConfigSchema } from "./search/core/config";
import { DEFAULT_JUDGE_MODEL, DEFAULT_STEP_LIMIT } from "./swe-atlas/schema";
import { BankingRetrievalConfigSchema } from "./tau3-bench-banking/retrieval-config";
import {
  DEFAULT_PI_PACKAGE,
  PI_THINKING_LEVELS,
} from "./terminal-bench/schema";
import { WandrOptionsSchema } from "./wandr/schema";

export const GEMINI_MEDIA_RESOLUTIONS = [
  "MEDIA_RESOLUTION_UNSPECIFIED",
  "MEDIA_RESOLUTION_LOW",
  "MEDIA_RESOLUTION_MEDIUM",
  "MEDIA_RESOLUTION_HIGH",
] as const;

export type GeminiMediaResolution = ValueOf<typeof GEMINI_MEDIA_RESOLUTIONS>;

export const InferenceOverrideSchema = z.object({
  temperature: z.number().optional(),
  maxTokens: z.number().optional(),
  reasoningEffort: z.enum(REASONING_EFFORTS).optional(),
  costTier: z.enum(COST_TIERS).optional(),
  timeoutMs: z.number().optional(),
  sort: z.nativeEnum(ProviderSort).optional(),
  cloudflareVersion: z.string().optional(),
  costQualityTradeoff: z.number().int().min(0).max(10).optional(),
  pinModel: z.boolean().optional(),
});

export type InferenceOverride = z.infer<typeof InferenceOverrideSchema>;

export type FixedTemperatureInferenceOverride = Omit<
  InferenceOverride,
  "temperature"
>;

export const ModelBenchmarkBaseSchema = z.object({
  model: z.string(),
  endpointId: z.string().optional(),
  ...InferenceOverrideSchema.shape,
  maxRetries: z.number().optional(),
});

export const FixedTemperatureBenchmarkBaseSchema =
  ModelBenchmarkBaseSchema.omit({
    temperature: true,
  });

export const GpqaOptionsSchema = z.object({});

export const GpqaBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("gpqa_diamond"),
  ...FixedTemperatureBenchmarkBaseSchema.shape,
  ...GpqaOptionsSchema.shape,
});

export type GpqaBenchmarkConfig = z.infer<typeof GpqaBenchmarkConfigSchema>;

export const MmluProOptionsSchema = z.object({});

export const MmluProBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("mmlu_pro"),
  ...ModelBenchmarkBaseSchema.shape,
  ...MmluProOptionsSchema.shape,
});

export type MmluProBenchmarkConfig = z.infer<
  typeof MmluProBenchmarkConfigSchema
>;

export const TauBenchOptionsSchema = z.object({
  userModel: zDefaultedText(TAU_BENCH_AIRLINE_META.userModel),
});

export const TauBenchAirlineConfigSchema = z.object({
  benchmarkId: z.literal("tau_bench_verified_airline"),
  ...FixedTemperatureBenchmarkBaseSchema.shape,
  ...TauBenchOptionsSchema.shape,
});

export type TauBenchAirlineConfig = z.infer<typeof TauBenchAirlineConfigSchema>;

export const Tau3BenchBankingOptionsSchema = z.object({
  userModel: zDefaultedText(TAU3_BENCH_BANKING_META.userModel),
  userReasoningEffort: z.enum(REASONING_EFFORTS).default("medium"),
  retrievalConfig: BankingRetrievalConfigSchema,
});

export const Tau3BenchBankingConfigSchema = z.object({
  benchmarkId: z.literal("tau3_bench_banking"),
  ...FixedTemperatureBenchmarkBaseSchema.shape,
  ...Tau3BenchBankingOptionsSchema.shape,
});

export type Tau3BenchBankingConfig = z.infer<
  typeof Tau3BenchBankingConfigSchema
>;

export const MmmuProVisionOptionsSchema = z.object({
  imageDetail: z.enum(IMAGE_DETAIL_VALUES).optional(),
  mediaResolution: z.enum(GEMINI_MEDIA_RESOLUTIONS).optional(),
});

export const MmmuProVisionBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("mmmu_pro_vision"),
  ...ModelBenchmarkBaseSchema.shape,
  ...MmmuProVisionOptionsSchema.shape,
});

export type MmmuProVisionBenchmarkConfig = z.infer<
  typeof MmmuProVisionBenchmarkConfigSchema
>;

export const TerminalBenchOptionsSchema = z.object({
  maxAgentTimeoutSec: z.number().positive().optional(),
  taskSubset: z.array(z.string()).optional(),
  modalEnv: z.string().default("main"),
  thinking: z.enum(PI_THINKING_LEVELS).default("medium"),
  piPackage: z.string().default(DEFAULT_PI_PACKAGE),
  appendSystemPrompt: z.string().optional(),
});

export const TerminalBenchConfigSchema = z.object({
  benchmarkId: z.literal("terminal_bench"),
  ...ModelBenchmarkBaseSchema.shape,
  ...TerminalBenchOptionsSchema.shape,
});

export type TerminalBenchConfig = z.infer<typeof TerminalBenchConfigSchema>;

export const DracoBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("draco"),
  panelConfig: DracoPanelConfigSchema,
  artifactDir: z.string().optional(),
  maxRetries: z.number().optional(),
});

export type DracoBenchmarkConfig = z.infer<typeof DracoBenchmarkConfigSchema>;

export const IfStructOptionsSchema = z.object({});

export const IfStructBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("ifstruct"),
  ...ModelBenchmarkBaseSchema.shape,
  ...IfStructOptionsSchema.shape,
});

export type IfStructBenchmarkConfig = z.infer<
  typeof IfStructBenchmarkConfigSchema
>;

/**
 * Declarative custom eval: the run config carries the full EvalSpec (dataset +
 * prompt + deterministic scorer), so user evals are data, not registry code.
 */
export const CustomEvalOptionsSchema = z.object({
  spec: EvalSpecSchema,
});

export const CustomEvalBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("custom_eval"),
  ...ModelBenchmarkBaseSchema.shape,
  ...CustomEvalOptionsSchema.shape,
});
export type CustomEvalBenchmarkConfig = z.infer<
  typeof CustomEvalBenchmarkConfigSchema
>;

/** Chess has no per-run options: the five tasks are fixed dataset rows. */
export const ChessOptionsSchema = z.object({});

export const ChessBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("chess"),
  /* Fixed-temperature base: the solver pins temperature 0 for move
   * determinism, so the config must not accept (and silently drop) one. */
  ...FixedTemperatureBenchmarkBaseSchema.shape,
  ...ChessOptionsSchema.shape,
});
export type ChessBenchmarkConfig = z.infer<typeof ChessBenchmarkConfigSchema>;

/**
 * Options shared by the Modal-backed agentic benchmarks (SWE-Atlas tracks and
 * DeepSWE). `stepLimit` is declared per benchmark because the defaults differ.
 */
const AgenticOptionsSchema = z.object({
  taskSubset: z.array(z.string()).optional(),
  maxAgentTimeoutSec: z.number().positive().optional(),
  modalEnv: z.string().default("main"),
});

export const SweAtlasOptionsSchema = z.object({
  judgeModel: z.string().default(DEFAULT_JUDGE_MODEL),
  stepLimit: zInt().default(DEFAULT_STEP_LIMIT),
  ...AgenticOptionsSchema.shape,
});

export const SweAtlasQaConfigSchema = z.object({
  benchmarkId: z.literal("swe_atlas_qa"),
  ...ModelBenchmarkBaseSchema.shape,
  ...SweAtlasOptionsSchema.shape,
});

export type SweAtlasQaConfig = z.infer<typeof SweAtlasQaConfigSchema>;

export const SweAtlasTwConfigSchema = z.object({
  benchmarkId: z.literal("swe_atlas_tw"),
  ...ModelBenchmarkBaseSchema.shape,
  ...SweAtlasOptionsSchema.shape,
});

export type SweAtlasTwConfig = z.infer<typeof SweAtlasTwConfigSchema>;

export const SweAtlasRfConfigSchema = z.object({
  benchmarkId: z.literal("swe_atlas_rf"),
  ...ModelBenchmarkBaseSchema.shape,
  ...SweAtlasOptionsSchema.shape,
});

export type SweAtlasRfConfig = z.infer<typeof SweAtlasRfConfigSchema>;

export const DeepSweOptionsSchema = z.object({
  stepLimit: zInt().default(DEEP_SWE_DEFAULT_STEP_LIMIT),
  ...AgenticOptionsSchema.shape,
});

export const DeepSweConfigSchema = z.object({
  benchmarkId: z.literal("deep_swe"),
  ...ModelBenchmarkBaseSchema.shape,
  ...DeepSweOptionsSchema.shape,
});

export type DeepSweConfig = z.infer<typeof DeepSweConfigSchema>;

export const WandrConfigSchema = z.object({
  benchmarkId: z.literal("wandr"),
  ...ModelBenchmarkBaseSchema.shape,
  ...WandrOptionsSchema.shape,
});

export type WandrConfig = z.infer<typeof WandrConfigSchema>;

export const SearchBenchmarkOptionsSchema = z.object({
  lane: SearchLaneConfigSchema.default(
    () => ({ webSearch: "server-tool", engine: "auto" }) as const
  ),
});

export const BrowseCompBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("search_browsecomp"),
  ...ModelBenchmarkBaseSchema.shape,
  ...SearchBenchmarkOptionsSchema.shape,
});

export type BrowseCompBenchmarkConfig = z.infer<
  typeof BrowseCompBenchmarkConfigSchema
>;

export const HleBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("search_hle"),
  ...ModelBenchmarkBaseSchema.shape,
  ...SearchBenchmarkOptionsSchema.shape,
});

export type HleBenchmarkConfig = z.infer<typeof HleBenchmarkConfigSchema>;

export const DsqaBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("search_dsqa"),
  ...ModelBenchmarkBaseSchema.shape,
  ...SearchBenchmarkOptionsSchema.shape,
});

export type DsqaBenchmarkConfig = z.infer<typeof DsqaBenchmarkConfigSchema>;

export const WideSearchBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("search_widesearch"),
  ...ModelBenchmarkBaseSchema.shape,
  ...SearchBenchmarkOptionsSchema.shape,
});

export type WideSearchBenchmarkConfig = z.infer<
  typeof WideSearchBenchmarkConfigSchema
>;

export type SearchBenchmarkConfig =
  | BrowseCompBenchmarkConfig
  | HleBenchmarkConfig
  | DsqaBenchmarkConfig
  | WideSearchBenchmarkConfig;

export const BenchmarkRunConfigSchema = z.discriminatedUnion("benchmarkId", [
  GpqaBenchmarkConfigSchema,
  MmluProBenchmarkConfigSchema,
  TauBenchAirlineConfigSchema,
  Tau3BenchBankingConfigSchema,
  MmmuProVisionBenchmarkConfigSchema,
  TerminalBenchConfigSchema,
  DracoBenchmarkConfigSchema,
  IfStructBenchmarkConfigSchema,
  CustomEvalBenchmarkConfigSchema,
  ChessBenchmarkConfigSchema,
  SweAtlasQaConfigSchema,
  SweAtlasTwConfigSchema,
  SweAtlasRfConfigSchema,
  DeepSweConfigSchema,
  WandrConfigSchema,
  BrowseCompBenchmarkConfigSchema,
  HleBenchmarkConfigSchema,
  DsqaBenchmarkConfigSchema,
  WideSearchBenchmarkConfigSchema,
]);

export type BenchmarkRunConfig = z.infer<typeof BenchmarkRunConfigSchema>;

export type ModelBenchmarkConfig = Extract<
  BenchmarkRunConfig,
  {
    model: string;
  }
>;

export type ModelBenchmarkId = ModelBenchmarkConfig["benchmarkId"];

export function isModelBenchmarkConfig(
  config: BenchmarkRunConfig
): config is ModelBenchmarkConfig {
  return "model" in config;
}

export const BENCHMARK_OPTIONS_SCHEMAS = {
  gpqa_diamond: GpqaOptionsSchema,
  mmlu_pro: MmluProOptionsSchema,
  tau_bench_verified_airline: TauBenchOptionsSchema,
  tau3_bench_banking: Tau3BenchBankingOptionsSchema,
  mmmu_pro_vision: MmmuProVisionOptionsSchema,
  terminal_bench: TerminalBenchOptionsSchema,
  ifstruct: IfStructOptionsSchema,
  custom_eval: CustomEvalOptionsSchema,
  chess: ChessOptionsSchema,
  swe_atlas_qa: SweAtlasOptionsSchema,
  swe_atlas_tw: SweAtlasOptionsSchema,
  swe_atlas_rf: SweAtlasOptionsSchema,
  deep_swe: DeepSweOptionsSchema,
  wandr: WandrOptionsSchema,
  search_browsecomp: SearchBenchmarkOptionsSchema,
  search_hle: SearchBenchmarkOptionsSchema,
  search_dsqa: SearchBenchmarkOptionsSchema,
  search_widesearch: SearchBenchmarkOptionsSchema,
} as const satisfies Record<ModelBenchmarkId, z.ZodObject<z.ZodRawShape>>;

const SEARCH_BENCHMARK_ID_SET: ReadonlySet<string> = new Set([
  "search_browsecomp",
  "search_hle",
  "search_dsqa",
  "search_widesearch",
] satisfies readonly ModelBenchmarkId[]);

export function isSearchBenchmarkConfig(
  config: BenchmarkRunConfig
): config is SearchBenchmarkConfig {
  return SEARCH_BENCHMARK_ID_SET.has(config.benchmarkId);
}

export function modelFromConfig(
  config: BenchmarkRunConfig
): string | undefined {
  return isModelBenchmarkConfig(config) ? config.model : undefined;
}

export function endpointIdFromConfig(
  config: BenchmarkRunConfig
): string | undefined {
  return isModelBenchmarkConfig(config) ? config.endpointId : undefined;
}
