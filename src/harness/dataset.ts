import { Tag } from "effect/Context";
import type { Effect } from "effect/Effect";
import type { Stream } from "effect/Stream";

import type { DatasetError, Sample } from "./core";

export interface DatasetStreamOptions {
  readonly start?: number;
  readonly end?: number;
}

export class Dataset extends Tag("@openrouter/bench-harness/dataset")<
  Dataset,
  {
    readonly stream: (
      opts?: DatasetStreamOptions
    ) => Stream<Sample, DatasetError>;
    readonly size: Effect<number, DatasetError>;
  }
>() {}

export type DatasetService = {
  readonly stream: (
    opts?: DatasetStreamOptions
  ) => Stream<Sample, DatasetError>;
  readonly size: Effect<number, DatasetError>;
};
