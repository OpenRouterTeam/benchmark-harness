import { DRACO_BENCHMARK } from "./draco/benchmark";
import { GPQA_BENCHMARK } from "./gpqa";
import { IFSTRUCT_BENCHMARK } from "./ifstruct/benchmark";
import { MMLU_PRO_BENCHMARK } from "./mmlu-pro";
import { MMMU_PRO_VISION_BENCHMARK } from "./mmmu-pro-vision";
import { BROWSECOMP_BENCHMARK } from "./search/browsecomp/benchmark";
import { DSQA_BENCHMARK } from "./search/dsqa/benchmark";
import { HLE_BENCHMARK } from "./search/hle/benchmark";
import { WIDESEARCH_BENCHMARK } from "./search/widesearch/benchmark";
import { TAU_BENCH_AIRLINE_BENCHMARK } from "./tau-bench-airline/benchmark";
import { TAU3_BENCH_BANKING_BENCHMARK } from "./tau3-bench-banking/benchmark";
import type { Benchmark } from "./types";
import { VGI_BENCH_BENCHMARK } from "./vgi-bench/benchmark";

const BENCHMARKS: Record<string, Benchmark> = {
  [GPQA_BENCHMARK.id]: GPQA_BENCHMARK,
  [MMLU_PRO_BENCHMARK.id]: MMLU_PRO_BENCHMARK,
  [TAU_BENCH_AIRLINE_BENCHMARK.id]: TAU_BENCH_AIRLINE_BENCHMARK,
  [TAU3_BENCH_BANKING_BENCHMARK.id]: TAU3_BENCH_BANKING_BENCHMARK,
  [MMMU_PRO_VISION_BENCHMARK.id]: MMMU_PRO_VISION_BENCHMARK,
  [DRACO_BENCHMARK.id]: DRACO_BENCHMARK,
  [IFSTRUCT_BENCHMARK.id]: IFSTRUCT_BENCHMARK,
  [BROWSECOMP_BENCHMARK.id]: BROWSECOMP_BENCHMARK,
  [HLE_BENCHMARK.id]: HLE_BENCHMARK,
  [DSQA_BENCHMARK.id]: DSQA_BENCHMARK,
  [WIDESEARCH_BENCHMARK.id]: WIDESEARCH_BENCHMARK,
  [VGI_BENCH_BENCHMARK.id]: VGI_BENCH_BENCHMARK,
};

export function getBenchmark(id: string): Benchmark | undefined {
  return BENCHMARKS[id];
}

export function benchmarkIds(): readonly string[] {
  return Object.keys(BENCHMARKS);
}
