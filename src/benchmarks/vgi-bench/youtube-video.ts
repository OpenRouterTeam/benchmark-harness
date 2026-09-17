import { get as getContext, make } from "effect/Context";
import type { Layer } from "effect/Layer";
import { map as mapLayer } from "effect/Layer";
import { filter as streamFilter } from "effect/Stream";

import type { Sample } from "../../harness/core";
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

export function keepYoutubeSamples(layer: Layer<Dataset>): Layer<Dataset> {
  return mapLayer(layer, (context) => {
    const dataset = getContext(context, Dataset);
    return make(Dataset, {
      size: dataset.size,
      stream: (opts) =>
        dataset.stream(opts).pipe(streamFilter(isYoutubeSample)),
    });
  });
}
