import { describe, expect, it } from "bun:test";

import type { StageKey } from "./artifact-store";
import {
  FsArtifactStore,
  GcsArtifactStore,
  NoopArtifactStore,
} from "./artifact-store";
import { resolveArtifactStore } from "./benchmark";

const GEN_KEY: StageKey = { stage: "generation", key: "k" };

function gcsObjectPath(store: unknown): string {
  if (!(store instanceof GcsArtifactStore)) {
    throw new Error(
      `expected GcsArtifactStore, got ${store?.constructor?.name ?? String(store)}`
    );
  }
  return store.objectPath(GEN_KEY);
}
describe("resolveArtifactStore", () => {
  it("no args → no-op (default: no caching)", () => {
    expect(resolveArtifactStore(undefined, undefined)).toBeInstanceOf(
      NoopArtifactStore
    );
  });
  it("bucket env alone → no-op (no namespace = no reuse)", () => {
    expect(
      resolveArtifactStore(
        undefined,
        undefined,
        "openrouter-core-openbench-results-prod"
      )
    ).toBeInstanceOf(NoopArtifactStore);
  });
  it("artifactDir → FS store", () => {
    expect(
      resolveArtifactStore("/tmp/draco-run", undefined, "some-bucket")
    ).toBeInstanceOf(FsArtifactStore);
  });
  it("artifactDir wins over cacheNamespace + bucket", () => {
    expect(
      resolveArtifactStore("/tmp/draco-run", "sweep-1", "some-bucket")
    ).toBeInstanceOf(FsArtifactStore);
  });
  it("cacheNamespace + bucket → GCS at namespaced prefix", () => {
    const store = resolveArtifactStore(
      undefined,
      "sweep-1",
      "openrouter-core-openbench-results-prod"
    );
    expect(gcsObjectPath(store)).toBe(
      "draco/stage-cache/sweep-1/generation/k.json"
    );
  });
  it("different namespaces → different prefixes", () => {
    const a = resolveArtifactStore(undefined, "sweep-a", "bucket");
    const b = resolveArtifactStore(undefined, "sweep-b", "bucket");
    expect(gcsObjectPath(a)).not.toBe(gcsObjectPath(b));
  });
  it("cacheNamespace without bucket → no-op", () => {
    expect(
      resolveArtifactStore(undefined, "sweep-1", undefined)
    ).toBeInstanceOf(NoopArtifactStore);
  });
  it("empty-string cacheNamespace → no-op", () => {
    expect(resolveArtifactStore(undefined, "", "bucket")).toBeInstanceOf(
      NoopArtifactStore
    );
  });
  it("whitespace-only cacheNamespace → no-op (trimmed before use)", () => {
    expect(resolveArtifactStore(undefined, "   ", "bucket")).toBeInstanceOf(
      NoopArtifactStore
    );
  });
  it("surrounding whitespace is trimmed from the namespace", () => {
    expect(
      gcsObjectPath(resolveArtifactStore(undefined, " sweep-1 ", "bucket"))
    ).toBe("draco/stage-cache/sweep-1/generation/k.json");
  });
  it("empty-string bucket env → no-op", () => {
    expect(resolveArtifactStore(undefined, "sweep-1", "")).toBeInstanceOf(
      NoopArtifactStore
    );
  });
});
