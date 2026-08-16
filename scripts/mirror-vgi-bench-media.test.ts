import { describe, expect, test } from "bun:test";

import { VGI_BENCH_DEFAULT_REVISION } from "../src/benchmarks/vgi-bench/benchmark";
import type { ManifestEntry } from "./mirror-vgi-bench-media";
import {
  describeFailure,
  extensionOf,
  hashManifest,
  readOptions,
} from "./mirror-vgi-bench-media";

function makeEntry(overrides?: Partial<ManifestEntry>): ManifestEntry {
  return {
    videoId: "abc",
    sourceUrl: "https://example.test/abc_proxy_v2.mp4",
    sourceKind: "downscaled",
    key: "abc.mp4",
    url: "https://media.example.test/abc.mp4",
    bytes: 10,
    contentType: "video/mp4",
    sha256: "a".repeat(64),
    ...overrides,
  };
}

describe("extensionOf", () => {
  test("returns the lowercased extension", () => {
    expect(extensionOf("https://example.test/dir/clip.MP4?x=1")).toBe("mp4");
  });

  test("defaults to mp4 when the path has no extension", () => {
    expect(extensionOf("https://example.test/dir/clip")).toBe("mp4");
  });

  test("ignores dots in directory segments", () => {
    expect(extensionOf("https://example.test/v1.0/videos/clip")).toBe("mp4");
    expect(extensionOf("https://example.test/v1.0/videos/clip.webm")).toBe(
      "webm"
    );
  });
});

describe("readOptions", () => {
  test("applies defaults", () => {
    const options = readOptions([]);
    expect(options.concurrency).toBe(8);
    expect(options.revision).toBe(VGI_BENCH_DEFAULT_REVISION);
    expect(options.limit).toBeUndefined();
    expect(options.force).toBe(false);
    expect(options.dryRun).toBe(false);
  });

  test("parses provided flags", () => {
    const options = readOptions([
      "--concurrency=3",
      "--limit=5",
      "--out=manifest.json",
      "--revision=main",
      "--force",
      "--dry-run",
    ]);
    expect(options).toEqual({
      concurrency: 3,
      limit: 5,
      out: "manifest.json",
      revision: "main",
      force: true,
      dryRun: true,
    });
  });

  test("rejects invalid numeric flags", () => {
    expect(() => readOptions(["--concurrency=0"])).toThrow();
    expect(() => readOptions(["--limit=abc"])).toThrow();
  });
});

describe("describeFailure", () => {
  test("reports the message of an error", () => {
    expect(describeFailure(new Error("Download failed with 503"))).toBe(
      "Download failed with 503"
    );
  });

  test("stringifies non-error causes", () => {
    expect(describeFailure("socket hang up")).toBe("socket hang up");
  });
});

describe("hashManifest", () => {
  test("is stable for identical entries and changes with content", () => {
    const first = hashManifest([makeEntry()]);
    expect(hashManifest([makeEntry()])).toBe(first);
    expect(hashManifest([makeEntry({ sha256: "b".repeat(64) })])).not.toBe(
      first
    );
    expect(
      hashManifest([makeEntry({ url: "https://media.example.test/other.mp4" })])
    ).not.toBe(first);
  });
});
