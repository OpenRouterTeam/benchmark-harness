export interface BenchmarkMeta {
  readonly id: string;
  readonly defaultEpochs: number;
  readonly temperature?: number;
  readonly userModel?: string;
}

export const GPQA_META = {
  id: "gpqa_diamond",
  defaultEpochs: 10,
  temperature: 0.5,
} as const satisfies BenchmarkMeta;

export const MMLU_PRO_META = {
  id: "mmlu_pro",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const MMMU_PRO_VISION_META = {
  id: "mmmu_pro_vision",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const TAU_BENCH_AIRLINE_META = {
  id: "tau_bench_verified_airline",
  defaultEpochs: 1,
  temperature: 0,
  userModel: "google/gemini-2.5-flash",
} as const satisfies BenchmarkMeta;

export const TAU3_BENCH_BANKING_META = {
  id: "tau3_bench_banking",
  defaultEpochs: 5,
  userModel: "openai/gpt-5.4-mini",
} as const satisfies BenchmarkMeta;

export const TERMINAL_BENCH_META = {
  id: "terminal_bench",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const TERMINAL_BENCH_4_META = {
  id: "terminal_bench_4",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const DRACO_META = {
  id: "draco",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const IFSTRUCT_META = {
  id: "ifstruct",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const SWE_ATLAS_QA_META = {
  id: "swe_atlas_qa",
  defaultEpochs: 3,
} as const satisfies BenchmarkMeta;

export const SWE_ATLAS_TW_META = {
  id: "swe_atlas_tw",
  defaultEpochs: 3,
} as const satisfies BenchmarkMeta;

export const SWE_ATLAS_RF_META = {
  id: "swe_atlas_rf",
  defaultEpochs: 3,
} as const satisfies BenchmarkMeta;

export const DEEP_SWE_META = {
  id: "deep_swe",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const WANDR_META = {
  id: "wandr",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const BROWSECOMP_META = {
  id: "search_browsecomp",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const HLE_META = {
  id: "search_hle",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const DSQA_META = {
  id: "search_dsqa",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const WIDESEARCH_META = {
  id: "search_widesearch",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const VGI_BENCH_META = {
  id: "vgi_bench",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

const BENCHMARK_META: Readonly<Record<string, BenchmarkMeta>> = {
  [GPQA_META.id]: GPQA_META,
  [MMLU_PRO_META.id]: MMLU_PRO_META,
  [MMMU_PRO_VISION_META.id]: MMMU_PRO_VISION_META,
  [TAU_BENCH_AIRLINE_META.id]: TAU_BENCH_AIRLINE_META,
  [TAU3_BENCH_BANKING_META.id]: TAU3_BENCH_BANKING_META,
  [TERMINAL_BENCH_META.id]: TERMINAL_BENCH_META,
  [TERMINAL_BENCH_4_META.id]: TERMINAL_BENCH_4_META,
  [DRACO_META.id]: DRACO_META,
  [IFSTRUCT_META.id]: IFSTRUCT_META,
  [SWE_ATLAS_QA_META.id]: SWE_ATLAS_QA_META,
  [SWE_ATLAS_TW_META.id]: SWE_ATLAS_TW_META,
  [SWE_ATLAS_RF_META.id]: SWE_ATLAS_RF_META,
  [DEEP_SWE_META.id]: DEEP_SWE_META,
  [WANDR_META.id]: WANDR_META,
  [BROWSECOMP_META.id]: BROWSECOMP_META,
  [HLE_META.id]: HLE_META,
  [DSQA_META.id]: DSQA_META,
  [WIDESEARCH_META.id]: WIDESEARCH_META,
  [VGI_BENCH_META.id]: VGI_BENCH_META,
};

export function getBenchmarkMeta(id: string): BenchmarkMeta | undefined {
  return BENCHMARK_META[id];
}

export function benchmarkMetaIds(): readonly string[] {
  return Object.keys(BENCHMARK_META).sort();
}
