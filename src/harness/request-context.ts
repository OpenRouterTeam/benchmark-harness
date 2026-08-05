import type { Effect } from "effect/Effect";
import { get, set, unsafeMake } from "effect/FiberRef";

/**
 * Fiber-scoped (sample, epoch) identity of the evaluation currently issuing
 * model calls. The model layer stamps it into each request's `x_bench`
 * extension so the bench-gateway's request-coalescing hash can tell two
 * epochs of the same sample apart — without it, identical bodies from
 * different epochs coalesce and epoch 2 replays epoch 1's answer. Fiber-scoped
 * for the same reason as `generationIdCollector`: each (sample, epoch) runs
 * in its own child fiber under streamMapEffect concurrency.
 */
export interface BenchRequestContext {
  readonly sampleId: string;
  readonly epoch: number;
}

export const benchRequestContext = unsafeMake<BenchRequestContext | undefined>(
  undefined
);

export function setBenchRequestContext(
  context: BenchRequestContext
): Effect<void> {
  return set(benchRequestContext, context);
}

export const getBenchRequestContext: Effect<BenchRequestContext | undefined> =
  get(benchRequestContext);
