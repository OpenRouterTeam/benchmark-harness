import { Service as Tag } from "effect/Context";
import type { Effect } from "effect/Effect";
import type { Stream } from "effect/Stream";

import type { DatasetError, Sample } from "./core";

export interface DatasetStreamOptions {
  readonly start?: number;
  readonly end?: number;
}

export class Dataset extends Tag<
  Dataset,
  {
    readonly stream: (
      opts?: DatasetStreamOptions
    ) => Stream<Sample, DatasetError>;
    readonly size: Effect<number, DatasetError>;
  }
>()("@openrouter/bench-harness/dataset") {}

export type DatasetService = {
  readonly stream: (
    opts?: DatasetStreamOptions
  ) => Stream<Sample, DatasetError>;
  readonly size: Effect<number, DatasetError>;
};
