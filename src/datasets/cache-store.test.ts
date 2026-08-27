import { afterEach, describe, expect, it } from "bun:test";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { PassThrough, Readable } from "node:stream";

import type { GcsObjectClient } from "./cache-store";
import {
  makeGcsCacheStore,
  pipeTarGzTo,
  resolveCacheStore,
} from "./cache-store";

interface StoredObject {
  readonly content: Buffer;
  readonly contentType?: string;
  readonly updated: number;
}

function makeMemoryGcsClient(): GcsObjectClient & {
  readonly store: Map<string, StoredObject>;
  readonly setUpdated: (key: string, updated: number) => void;
} {
  const store = new Map<string, StoredObject>();
  return {
    store,
    setUpdated(key, updated) {
      const existing = store.get(key);
      if (existing !== undefined) {
        store.set(key, { ...existing, updated });
      }
    },
    async downloadObject(key) {
      const obj = store.get(key);
      return obj === undefined ? undefined : Buffer.from(obj.content);
    },
    async uploadObject(key, content, contentType) {
      store.set(key, {
        content: Buffer.from(content),
        contentType,
        updated: Date.now(),
      });
    },
    async openObjectReadStream(key) {
      const obj = store.get(key);
      return obj === undefined ? undefined : Readable.from(obj.content);
    },
    openObjectWriteStream(key, contentType) {
      const stream = new PassThrough();
      const chunks: Buffer[] = [];
      stream.on("data", (chunk: Buffer) => chunks.push(Buffer.from(chunk)));
      stream.on("finish", () => {
        store.set(key, {
          content: Buffer.concat(chunks),
          contentType,
          updated: Date.now(),
        });
      });
      stream.on("error", () => {});
      return stream;
    },
    async objectUpdatedMs(key) {
      const obj = store.get(key);
      return obj === undefined ? undefined : obj.updated;
    },
  };
}

describe("GcsCacheStore", () => {
  const savedBackend = process.env.BENCH_DATASET_CACHE_BACKEND;
  const savedDisable = process.env.BENCH_DATASET_CACHE_DISABLE;
  const tmpDirs: string[] = [];
  afterEach(() => {
    for (const name of [
      "BENCH_DATASET_CACHE_BACKEND",
      "BENCH_DATASET_CACHE_DISABLE",
    ] as const) {
      delete process.env[name];
    }
    if (savedBackend !== undefined) {
      process.env.BENCH_DATASET_CACHE_BACKEND = savedBackend;
    }
    if (savedDisable !== undefined) {
      process.env.BENCH_DATASET_CACHE_DISABLE = savedDisable;
    }
    while (tmpDirs.length > 0) {
      const dir = tmpDirs.pop();
      if (dir !== undefined) {
        rmSync(dir, { recursive: true, force: true });
      }
    }
  });

  function makeTmpDir(): string {
    const dir = mkdtempSync(join(tmpdir(), "gcs-cache-test-"));
    tmpDirs.push(dir);
    return dir;
  }

  it("readJson/writeJson round-trip under the configured prefix", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({
      bucket: "b",
      prefix: "bench/harness",
      client,
    });
    process.env.BENCH_DATASET_CACHE_DISABLE = "0";
    expect(store.backend).toBe("gcs");
    expect(await store.readJson("hf/anon/x.json")).toBeUndefined();
    await store.writeJson("hf/anon/x.json", { ok: 1 });
    expect(await store.readJson("hf/anon/x.json")).toEqual({ ok: 1 });
    expect(client.store.has("bench/harness/hf/anon/x.json")).toBe(true);
    const obj = client.store.get("bench/harness/hf/anon/x.json")!;
    expect(obj.contentType).toBe("application/json");
  });

  it("never expires entries with no maxAgeMs (pinned revisions)", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({ bucket: "b", client });
    await store.writeJson("hf/anon/rev/0-1.json", { n: 1 });
    client.setUpdated(
      "hf/anon/rev/0-1.json",
      Date.now() - 365 * 24 * 60 * 60 * 1e3
    );
    expect(await store.readJson("hf/anon/rev/0-1.json")).toEqual({ n: 1 });
  });

  it("expires entries past maxAgeMs (HEAD pages) and serves fresh ones", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({ bucket: "b", client });
    await store.writeJson("hf/anon/HEAD/0-1.json", { n: 1 });
    const stale = Date.now() - 48 * 60 * 60 * 1e3;
    client.setUpdated("hf/anon/HEAD/0-1.json", stale);
    expect(
      await store.readJson("hf/anon/HEAD/0-1.json", {
        maxAgeMs: 24 * 60 * 60 * 1e3,
        now: Date.now(),
      })
    ).toBeUndefined();
    await store.writeJson("hf/anon/HEAD/0-1.json", { n: 2 });
    expect(
      await store.readJson("hf/anon/HEAD/0-1.json", {
        maxAgeMs: 24 * 60 * 60 * 1e3,
        now: Date.now(),
      })
    ).toEqual({ n: 2 });
  });

  it("treats download failures as cache misses", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({ bucket: "b", client });
    expect(await store.readJson("missing.json")).toBeUndefined();
  });

  it("snapshotCheckout + tryHydrateCheckout round-trip a directory tree", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({
      bucket: "b",
      prefix: "bench",
      client,
    });
    const src = makeTmpDir();
    writeFileSync(join(src, "task.toml"), "name = 'a'\n");
    mkdirSync(join(src, "nested"), { recursive: true });
    writeFileSync(join(src, "nested", "data.txt"), "hello\n");
    await store.snapshotCheckout(src, "harbor", "deadbeefdeadbeef");
    const snapshotKey = "bench/repos/harbor-deadbeefdead.tar.gz";
    expect(client.store.has(snapshotKey)).toBe(true);
    expect(client.store.get(snapshotKey)!.contentType).toBe("application/gzip");

    const dest = makeTmpDir();
    const hydrated = await store.tryHydrateCheckout(
      dest,
      "harbor",
      "deadbeefdeadbeef"
    );
    expect(hydrated).toBe(true);
    expect(existsSync(join(dest, "task.toml"))).toBe(true);
    expect(readFileSync(join(dest, "task.toml"), "utf8")).toBe("name = 'a'\n");
    expect(readFileSync(join(dest, "nested", "data.txt"), "utf8")).toBe(
      "hello\n"
    );
  });

  it("tryHydrateCheckout returns false when no snapshot exists", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({ bucket: "b", client });
    const dir = makeTmpDir();
    await expect(
      store.tryHydrateCheckout(dir, "harbor", "abc123")
    ).resolves.toBe(false);
  });

  it("aborts the upload when tar exits nonzero", async () => {
    const client = makeMemoryGcsClient();
    const dir = makeTmpDir();
    const snapshotKey = "repos/harbor-missing.tar.gz";
    await expect(
      pipeTarGzTo(
        join(dir, "missing"),
        client.openObjectWriteStream(snapshotKey, "application/gzip")
      )
    ).rejects.toThrow("tar create exited");
    expect(client.store.has(snapshotKey)).toBe(false);
  });

  it("does not export full-buffer tar helpers", async () => {
    const cacheStore = await import("./cache-store");
    expect("createTarGz" in cacheStore).toBe(false);
    expect("extractTarGz" in cacheStore).toBe(false);
  });
});

describe("resolveCacheStore", () => {
  const saved = {
    BENCH_DATASET_CACHE_BACKEND: process.env.BENCH_DATASET_CACHE_BACKEND,
    BENCH_GCS_BUCKET: process.env.BENCH_GCS_BUCKET,
  };
  afterEach(() => {
    for (const [name, value] of Object.entries(saved)) {
      delete process.env[name];
      if (value !== undefined) {
        process.env[name] = value;
      }
    }
  });

  it("defaults to disk", () => {
    delete process.env.BENCH_DATASET_CACHE_BACKEND;
    expect(resolveCacheStore().backend).toBe("disk");
  });
  it("falls back to disk when backend=gcs but no bucket is set", () => {
    process.env.BENCH_DATASET_CACHE_BACKEND = "gcs";
    delete process.env.BENCH_GCS_BUCKET;
    expect(resolveCacheStore().backend).toBe("disk");
  });
  it("selects gcs when backend and bucket are set", () => {
    process.env.BENCH_DATASET_CACHE_BACKEND = "gcs";
    process.env.BENCH_GCS_BUCKET = "my-bucket";
    expect(resolveCacheStore().backend).toBe("gcs");
  });
});
