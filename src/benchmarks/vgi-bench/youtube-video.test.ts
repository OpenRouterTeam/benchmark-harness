import { describe, expect, it } from "bun:test";

import { flatMap, provide, runPromise, succeed } from "effect/Effect";
import { effect as layerEffect } from "effect/Layer";
import { fromIterable, runCollect } from "effect/Stream";

import type { Sample } from "../../harness/core";
import { Dataset } from "../../harness/dataset";
import {
  isYoutubeSample,
  keepYoutubeSamples,
  youtubeVideoUrl,
} from "./youtube-video";

function sample(id: string, videoSource?: string): Sample {
  return {
    id,
    input: "q",
    target: { text: "A" },
    metadata: videoSource === undefined ? {} : { video_source: videoSource },
  };
}

describe("youtubeVideoUrl", () => {
  it("builds a watch URL for an 11-character YouTube id", () => {
    expect(youtubeVideoUrl("BEIWgGUcz2o")).toBe(
      "https://www.youtube.com/watch?v=BEIWgGUcz2o"
    );
    expect(youtubeVideoUrl("_-zi-NfJx0M")).toBe(
      "https://www.youtube.com/watch?v=_-zi-NfJx0M"
    );
  });

  it("returns undefined for synthetic clip ids", () => {
    expect(youtubeVideoUrl("compression-bench-falling")).toBeUndefined();
    expect(youtubeVideoUrl("clip_007")).toBeUndefined();
    expect(youtubeVideoUrl("BEIWgGUcz2o.mp4")).toBeUndefined();
  });
});

describe("keepYoutubeSamples", () => {
  it("drops samples whose video_source is not youtube", async () => {
    const samples = [
      sample("a", "youtube"),
      sample("b"),
      sample("c", "mirror"),
      sample("d", "youtube"),
    ];
    const base = layerEffect(
      Dataset,
      succeed({
        size: succeed(samples.length),
        stream: () => fromIterable(samples),
      })
    );
    const collected = await runPromise(
      Dataset.pipe(
        flatMap((dataset) => runCollect(dataset.stream())),
        provide(keepYoutubeSamples(base))
      )
    );
    expect([...collected].map((s) => s.id)).toEqual(["a", "d"]);
    expect(samples.filter(isYoutubeSample)).toHaveLength(2);
  });
});
