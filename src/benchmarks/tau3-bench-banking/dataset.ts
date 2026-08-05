import { FetchHttpClient, HttpClient } from "@effect/platform";
import {
  gen,
  makeSemaphore,
  map,
  mapError,
  provideService,
  retry,
} from "effect/Effect";
import type { Layer as LayerType } from "effect/Layer";
import { effect as layerEffect, provide as layerProvide } from "effect/Layer";
import type { Stream as StreamType } from "effect/Stream";
import {
  fail,
  flatMap as streamFlatMap,
  fromEffect,
  fromIterable,
} from "effect/Stream";

import { hfFetchRetrySchedule } from "../../datasets/huggingface";
import type { Sample } from "../../harness/core";
import { DatasetError } from "../../harness/core";
import type { DatasetStreamOptions } from "../../harness/dataset";
import { Dataset } from "../../harness/dataset";
import type { RetryConfig } from "../../runtime/retry";
import { ensureBankingTasks, loadBankingTasks } from "./environment";
import type { Tau3Task } from "./types";

export function bankingRecordToSample(task: Tau3Task): Sample {
  return {
    id: `tau3_bench_banking-${task.id}`,
    input: task.user_scenario.instructions,
    target: { text: "" },
    metadata: { task },
  };
}

export function makeBankingDatasetLayer(
  retryConfig?: RetryConfig
): LayerType<Dataset> {
  const makeService = gen(function* () {
    const client = yield* HttpClient.HttpClient;
    const fetchLock = yield* makeSemaphore(1);
    const loadTasks = ensureBankingTasks(fetchLock).pipe(
      provideService(HttpClient.HttpClient, client),
      retry(hfFetchRetrySchedule(retryConfig)),
      mapError(
        (cause) =>
          new DatasetError({
            message: `Failed to load tau3 banking tasks: ${String(cause)}`,
          })
      ),
      map(() => loadBankingTasks())
    );
    const stream = (
      opts?: DatasetStreamOptions
    ): StreamType<Sample, DatasetError> => {
      return fromEffect(loadTasks).pipe(
        streamFlatMap((tasks) => {
          const start = opts?.start ?? 0;
          const end = opts?.end ?? tasks.length;
          if (start < 0 || end > tasks.length || start > end) {
            return fail(
              new DatasetError({
                message: `Invalid stream range [${start}, ${end}) for dataset size ${tasks.length}`,
              })
            );
          }
          return fromIterable(
            tasks.slice(start, end).map(bankingRecordToSample)
          );
        })
      );
    };
    return Dataset.of({
      stream,
      size: loadTasks.pipe(map((tasks) => tasks.length)),
    });
  });
  return layerEffect(Dataset, makeService).pipe(
    layerProvide(FetchHttpClient.layer)
  );
}
