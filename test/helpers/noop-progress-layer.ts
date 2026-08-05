import { succeed as layerSucceed } from "effect/Layer";

import {
  CheckpointStore,
  NOOP_CHECKPOINT_STORE,
  NOOP_PROGRESS_REPORTER,
  ProgressReporter,
} from "../../src/harness/progress";

export const noopProgressLayer = layerSucceed(
  ProgressReporter,
  NOOP_PROGRESS_REPORTER
);

export const noopCheckpointLayer = layerSucceed(
  CheckpointStore,
  NOOP_CHECKPOINT_STORE
);
