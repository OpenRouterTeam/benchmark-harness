import { get as getContext, make } from "effect/Context";
import type { Layer } from "effect/Layer";
import { map as mapLayer } from "effect/Layer";
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
  return mapLayer(layer, (context) => {
    const dataset = getContext(context, Dataset);
    const filtered = dataset.stream().pipe(streamFilter(isYoutubeSample));
    const stream: Dataset["Type"]["stream"] = (opts) =>
      sliceStream(filtered, opts);
    return make(Dataset, { size: runCount(filtered), stream });
  });
}
