import {
  COST_TIERS,
  IMAGE_DETAIL_VALUES,
  REASONING_EFFORTS,
  stripVariantSuffix,
  SWITCHYARD_ALGORITHMS,
  VIDEO_PROCESSING_MODES,
} from "../harness/constants";
import { ProviderSort } from "../internal/enums";
import type { ValueOf } from "../internal/guards";
import { z, zDefaultedText } from "../internal/zod";
import { SWITCHYARD_MODEL } from "../providers/switchyard-router-plugin";
import {
  TAU3_BENCH_BANKING_META,
  TAU_BENCH_AIRLINE_META,
} from "./benchmark-meta";
import { DracoPanelConfigSchema } from "./draco/schemas";
import { SearchLaneConfigSchema } from "./search/core/config";
import { BankingRetrievalConfigSchema } from "./tau3-bench-banking/retrieval-config";

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
  reasoningEffort: z.enum(REASONING_EFFORTS),
  costTier: z.enum(COST_TIERS).optional(),
  timeoutMs: z.number().optional(),
  sort: z.nativeEnum(ProviderSort).optional(),
  providerOnly: z.array(z.string()).optional(),
  providerIgnore: z.array(z.string()).optional(),
  allowFallbacks: z.boolean().optional(),
  cloudflareVersion: z.string().optional(),
  costQualityTradeoff: z.number().int().min(0).max(10).optional(),
  pinModel: z.boolean().optional(),
  switchyardAlgorithm: z.enum(SWITCHYARD_ALGORITHMS).optional(),
});

export type InferenceOverride = z.infer<typeof InferenceOverrideSchema>;

export type FixedTemperatureInferenceOverride = Omit<
  InferenceOverride,
  "temperature"
>;

export const ModelBenchmarkBaseSchema = z.object({
  model: z.string(),
  models: z.array(z.string().min(1)).optional(),
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
  userReasoningEffort: z.enum(REASONING_EFFORTS).default("medium"),
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
  datasetRevision: z.string().optional(),
});

export const MmmuProVisionBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("mmmu_pro_vision"),
  ...ModelBenchmarkBaseSchema.shape,
  ...MmmuProVisionOptionsSchema.shape,
});

export type MmmuProVisionBenchmarkConfig = z.infer<
  typeof MmmuProVisionBenchmarkConfigSchema
>;

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

export const SearchBenchmarkOptionsSchema = z.object({
  lane: SearchLaneConfigSchema.default(
    () => ({ webSearch: "server-tool", engine: "auto" }) as const
  ),
  providerOrder: z.array(z.string()).optional(),
  providerOnly: z.array(z.string()).optional(),
  allowFallbacks: z.boolean().optional(),
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

export const VgiBenchOptionsSchema = z.object({
  downscaledVideos: z.boolean().default(false),
  videoProcessing: z.enum(VIDEO_PROCESSING_MODES).optional(),
  youtubeVideos: z.boolean().default(false),
  datasetRevision: z.string().optional(),
});

export const VgiBenchmarkConfigSchema = z.object({
  benchmarkId: z.literal("vgi_bench"),
  ...FixedTemperatureBenchmarkBaseSchema.shape,
  ...VgiBenchOptionsSchema.shape,
});

export type VgiBenchmarkConfig = z.infer<typeof VgiBenchmarkConfigSchema>;

export type SearchBenchmarkConfig =
  | BrowseCompBenchmarkConfig
  | HleBenchmarkConfig
  | DsqaBenchmarkConfig
  | WideSearchBenchmarkConfig;

export const NativeBenchmarkRunConfigSchema = z.discriminatedUnion(
  "benchmarkId",
  [
    GpqaBenchmarkConfigSchema,
    MmluProBenchmarkConfigSchema,
    TauBenchAirlineConfigSchema,
    Tau3BenchBankingConfigSchema,
    MmmuProVisionBenchmarkConfigSchema,
    DracoBenchmarkConfigSchema,
    IfStructBenchmarkConfigSchema,
    BrowseCompBenchmarkConfigSchema,
    HleBenchmarkConfigSchema,
    DsqaBenchmarkConfigSchema,
    WideSearchBenchmarkConfigSchema,
    VgiBenchmarkConfigSchema,
  ]
);

export type NativeBenchmarkRunConfig = z.infer<
  typeof NativeBenchmarkRunConfigSchema
>;

type NativeModelBenchmarkConfig = Extract<
  NativeBenchmarkRunConfig,
  {
    model: string;
  }
>;

export type ModelBenchmarkId = NativeModelBenchmarkConfig["benchmarkId"];

export const BENCHMARK_OPTIONS_SCHEMAS = {
  gpqa_diamond: GpqaOptionsSchema,
  mmlu_pro: MmluProOptionsSchema,
  tau_bench_verified_airline: TauBenchOptionsSchema,
  tau3_bench_banking: Tau3BenchBankingOptionsSchema,
  mmmu_pro_vision: MmmuProVisionOptionsSchema,
  ifstruct: IfStructOptionsSchema,
  search_browsecomp: SearchBenchmarkOptionsSchema,
  search_hle: SearchBenchmarkOptionsSchema,
  search_dsqa: SearchBenchmarkOptionsSchema,
  search_widesearch: SearchBenchmarkOptionsSchema,
  vgi_bench: VgiBenchOptionsSchema,
} as const satisfies Record<ModelBenchmarkId, z.ZodObject<z.ZodRawShape>>;

const NATIVE_BENCHMARK_ID_SET: ReadonlySet<string> = new Set(
  NativeBenchmarkRunConfigSchema.options.flatMap((schema) => {
    const benchmarkId = schema.shape.benchmarkId;
    return benchmarkId instanceof z.ZodLiteral ? [benchmarkId.value] : [];
  })
);

const InjectedBenchmarkIdSchema = z
  .string()
  .min(1)
  .refine(
    (benchmarkId) => !NATIVE_BENCHMARK_ID_SET.has(benchmarkId),
    "Injected benchmark ids must not reuse native benchmark ids"
  );

export const InjectedBenchmarkRunConfigSchema = z.object({
  benchmarkId: InjectedBenchmarkIdSchema,
  ...ModelBenchmarkBaseSchema.shape,
  options: z.record(z.string(), z.unknown()).default({}),
});

export type InjectedBenchmarkRunConfig = z.infer<
  typeof InjectedBenchmarkRunConfigSchema
>;

export const BenchmarkRunConfigSchema = z
  .union([NativeBenchmarkRunConfigSchema, InjectedBenchmarkRunConfigSchema])
  .refine(
    (config) =>
      !("model" in config) ||
      config.switchyardAlgorithm === undefined ||
      stripVariantSuffix(config.model) === SWITCHYARD_MODEL,
    {
      message: `requires model ${SWITCHYARD_MODEL}`,
      path: ["switchyardAlgorithm"],
    }
  );

export type BenchmarkRunConfig = z.infer<typeof BenchmarkRunConfigSchema>;

export type ModelBenchmarkConfig =
  | NativeModelBenchmarkConfig
  | InjectedBenchmarkRunConfig;

export function isModelBenchmarkConfig(
  config: BenchmarkRunConfig
): config is ModelBenchmarkConfig {
  return "model" in config;
}

export function isNativeBenchmarkConfig(
  config: BenchmarkRunConfig
): config is NativeBenchmarkRunConfig {
  return NATIVE_BENCHMARK_ID_SET.has(config.benchmarkId);
}

export function isInjectedBenchmarkConfig(
  config: BenchmarkRunConfig
): config is InjectedBenchmarkRunConfig {
  return !isNativeBenchmarkConfig(config);
}

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

export function knownBenchmarkOptionKeys(
  benchmarkId: ModelBenchmarkId
): ReadonlySet<string> {
  return new Set([
    ...Object.keys(BENCHMARK_OPTIONS_SCHEMAS[benchmarkId].shape),
    ...Object.keys(ModelBenchmarkBaseSchema.shape),
    "benchmarkId",
  ]);
}

export function isModelBenchmarkId(id: string): id is ModelBenchmarkId {
  return Object.hasOwn(BENCHMARK_OPTIONS_SCHEMAS, id);
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
