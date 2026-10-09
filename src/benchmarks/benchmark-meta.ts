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

export const TAU_BENCH_AIRLINE_META = {
  id: "tau_bench_verified_airline",
  defaultEpochs: 1,
  temperature: 0,
  userModel: "google/gemini-2.5-flash",
} as const satisfies BenchmarkMeta;

export const DRACO_META = {
  id: "draco",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const IFSTRUCT_META = {
  id: "ifstruct",
  defaultEpochs: 1,
} as const satisfies BenchmarkMeta;

export const TOOLCALL_FORMATS_META = {
  id: "toolcall_formats",
  defaultEpochs: 1,
  temperature: 0,
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
  [TAU_BENCH_AIRLINE_META.id]: TAU_BENCH_AIRLINE_META,
  [DRACO_META.id]: DRACO_META,
  [IFSTRUCT_META.id]: IFSTRUCT_META,
  [TOOLCALL_FORMATS_META.id]: TOOLCALL_FORMATS_META,
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
