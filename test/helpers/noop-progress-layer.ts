import { succeed as layerSucceed } from "effect/Layer";

import {
  CheckpointStore,
  NOOP_CHECKPOINT_STORE,
  NOOP_PROGRESS_REPORTER,
  ProgressReporter,
} from "../../src/harness/progress";
import {
  NOOP_SAMPLE_RESULT_STORE,
  SampleResultStore,
} from "../../src/harness/sample-result-store";

export const noopProgressLayer = layerSucceed(
  ProgressReporter,
  NOOP_PROGRESS_REPORTER
);

export const noopCheckpointLayer = layerSucceed(
  CheckpointStore,
  NOOP_CHECKPOINT_STORE
);
export const noopSampleResultLayer = layerSucceed(
  SampleResultStore,
  NOOP_SAMPLE_RESULT_STORE
);
