import { describe, expect, it } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { provide, runPromise } from "effect/Effect";

import { assertRight } from "../../internal/testing";
import type { StageKey } from "./artifact-store";
import {
  ArtifactStoreService,
  FsArtifactStore,
  NoopArtifactStore,
  makeArtifactStoreLayer,
  makeFsArtifactStoreLayer,
  makeNoopArtifactStoreLayer,
  stageGet,
  stagePut,
} from "./artifact-store";

const KEY: StageKey = { stage: "judge", key: "abc123" };
describe("FsArtifactStore", () => {
  it("returns undefined for a missing key (cache miss)", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-store-"));
    try {
      const store = new FsArtifactStore(dir);
      const r = await store.get(KEY);
      assertRight(r);
      expect(r.right).toBeUndefined();
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("round-trips a put then get", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-store-"));
    try {
      const store = new FsArtifactStore(dir);
      assertRight(await store.put(KEY, '{"verdict":"MET"}'));
      const got = await store.get(KEY);
      assertRight(got);
      expect(got.right).toBe('{"verdict":"MET"}');
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("put is idempotent (overwrite same key)", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-store-"));
    try {
      const store = new FsArtifactStore(dir);
      assertRight(await store.put(KEY, "v1"));
      assertRight(await store.put(KEY, "v2"));
      const got = await store.get(KEY);
      assertRight(got);
      expect(got.right).toBe("v2");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
describe("NoopArtifactStore", () => {
  it("always misses and never persists", async () => {
    const store = new NoopArtifactStore();
    const got = await store.get(KEY);
    assertRight(got);
    expect(got.right).toBeUndefined();
    assertRight(await store.put(KEY, "x"));
    const got2 = await store.get(KEY);
    assertRight(got2);
    expect(got2.right).toBeUndefined();
  });
});
describe("Effect stageGet / stagePut", () => {
  it("get/put through the ArtifactStoreService layer round-trip a stage", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-layer-"));
    try {
      const layer = makeFsArtifactStoreLayer(dir);
      await runPromise(stagePut(KEY, '{"v":1}').pipe(provide(layer)));
      const got = await runPromise(stageGet(KEY).pipe(provide(layer)));
      expect(got).toBe('{"v":1}');
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("stageGet returns undefined for a miss through the layer", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-layer-"));
    try {
      const layer = makeFsArtifactStoreLayer(dir);
      const got = await runPromise(
        stageGet({ stage: "generation", key: "nope" }).pipe(provide(layer))
      );
      expect(got).toBeUndefined();
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("noop layer never persists", async () => {
    const layer = makeNoopArtifactStoreLayer();
    await runPromise(stagePut(KEY, "x").pipe(provide(layer)));
    const got = await runPromise(stageGet(KEY).pipe(provide(layer)));
    expect(got).toBeUndefined();
  });
  it("makeArtifactStoreLayer wraps any ArtifactStore impl", async () => {
    const layer = makeArtifactStoreLayer(new NoopArtifactStore());
    const got = await runPromise(stageGet(KEY).pipe(provide(layer)));
    expect(got).toBeUndefined();
    expect(ArtifactStoreService.key).toBe(
      "@openrouter/bench-harness/benchmarks/draco/artifact-store/ArtifactStoreService"
    );
  });
});
