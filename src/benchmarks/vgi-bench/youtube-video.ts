import { map as mapEffect } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect as layerEffect, provide as layerProvide } from "effect/Layer";
import type { Stream } from "effect/Stream";
import {
  drop as streamDrop,
  filter as streamFilter,
  runCount,
  take as streamTake,
} from "effect/Stream";

import type { DatasetError, Sample } from "../../harness/core";
import type { DatasetStreamOptions } from "../../harness/dataset";
import { Dataset } from "../../harness/dataset";

export const VGI_BENCH_VIDEO_SOURCE_YOUTUBE = "youtube";

const YOUTUBE_VIDEO_ID = /^[A-Za-z0-9_-]{11}$/;

export function youtubeVideoUrl(videoId: string): string | undefined {
  if (!YOUTUBE_VIDEO_ID.test(videoId)) {
    return undefined;
  }
  return `https://www.youtube.com/watch?v=${videoId}`;
}

export function isYoutubeSample(sample: Sample): boolean {
  return sample.metadata?.["video_source"] === VGI_BENCH_VIDEO_SOURCE_YOUTUBE;
}

function sliceStream(
  source: Stream<Sample, DatasetError>,
  opts: DatasetStreamOptions | undefined
): Stream<Sample, DatasetError> {
  const start = opts?.start ?? 0;
  const dropped = start > 0 ? source.pipe(streamDrop(start)) : source;
  return opts?.end === undefined
    ? dropped
    : dropped.pipe(streamTake(Math.max(0, opts.end - start)));
}

export function keepYoutubeSamples(layer: Layer<Dataset>): Layer<Dataset> {
  return layerEffect(
    Dataset,
    mapEffect(Dataset, (dataset) => {
      const filtered = dataset.stream().pipe(streamFilter(isYoutubeSample));
      const stream: Dataset["Service"]["stream"] = (opts) =>
        sliceStream(filtered, opts);
      return { size: runCount(filtered), stream };
    })
  ).pipe(layerProvide(layer));
}
