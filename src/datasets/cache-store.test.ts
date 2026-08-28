import { afterEach, describe, expect, it, spyOn } from "bun:test";
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
  makeDiskCacheStore,
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
  const savedLogLevel = process.env.LOG_LEVEL;
  const savedOrEnv = process.env.OR_ENV;
  const tmpDirs: string[] = [];
  const infoSpies: { mockRestore: () => void }[] = [];
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
    if (savedLogLevel === undefined) {
      delete process.env.LOG_LEVEL;
    } else {
      process.env.LOG_LEVEL = savedLogLevel;
    }
    if (savedOrEnv === undefined) {
      delete process.env.OR_ENV;
    } else {
      process.env.OR_ENV = savedOrEnv;
    }
    for (const info of infoSpies.splice(0)) {
      info.mockRestore();
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

  function captureInfo() {
    process.env.OR_ENV = "development";
    process.env.LOG_LEVEL = "1";
    const info = spyOn(console, "info").mockImplementation(() => {});
    infoSpies.push(info);
    return info;
  }

  it("logs a hit when a fresh JSON object is read", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({ bucket: "b", client });
    await store.writeJson("hit.json", { ok: true });
    const info = captureInfo();

    await expect(store.readJson("hit.json")).resolves.toEqual({ ok: true });

    expect(info.mock.calls).toEqual([
      [
        "dataset cache read",
        { backend: "gcs", key: "hit.json", result: "hit" },
      ],
    ]);
  });

  it("logs a miss when a JSON object is absent", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({ bucket: "b", client });
    const info = captureInfo();

    await expect(store.readJson("missing.json")).resolves.toBeUndefined();

    expect(info.mock.calls).toEqual([
      [
        "dataset cache read",
        { backend: "gcs", key: "missing.json", result: "miss" },
      ],
    ]);
  });

  it("logs stale when a JSON object exceeds maxAgeMs", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({ bucket: "b", client });
    await store.writeJson("stale.json", { ok: true });
    client.setUpdated("stale.json", 1_000);
    const info = captureInfo();

    await expect(
      store.readJson("stale.json", { maxAgeMs: 1_000, now: 3_000 })
    ).resolves.toBeUndefined();

    expect(info.mock.calls).toEqual([
      [
        "dataset cache read",
        { backend: "gcs", key: "stale.json", result: "stale" },
      ],
    ]);
  });

  it("logs invalid when a JSON object cannot be parsed", async () => {
    const client = makeMemoryGcsClient();
    client.store.set("invalid.json", {
      content: Buffer.from("{"),
      updated: Date.now(),
    });
    const store = makeGcsCacheStore({ bucket: "b", client });
    const info = captureInfo();

    await expect(store.readJson("invalid.json")).resolves.toBeUndefined();

    expect(info.mock.calls).toEqual([
      [
        "dataset cache read",
        { backend: "gcs", key: "invalid.json", result: "invalid" },
      ],
    ]);
  });

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

  it("logs a checkout hydrate hit after extraction succeeds", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({ bucket: "b", client });
    const src = makeTmpDir();
    writeFileSync(join(src, "task.toml"), "name = 'a'\n");
    await store.snapshotCheckout(src, "harbor", "deadbeefdeadbeef");
    const info = captureInfo();
    const dest = makeTmpDir();

    await expect(
      store.tryHydrateCheckout(dest, "harbor", "deadbeefdeadbeef")
    ).resolves.toBe(true);

    expect(info.mock.calls).toEqual([
      [
        "checkout cache hydrate",
        {
          key: "repos/harbor-deadbeefdead.tar.gz",
          scope: "harbor",
          commit: "deadbeefdeadbeef",
          result: "hit",
        },
      ],
    ]);
  });

  it("tryHydrateCheckout returns false when no snapshot exists", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({ bucket: "b", client });
    const dir = makeTmpDir();
    const info = captureInfo();

    await expect(
      store.tryHydrateCheckout(dir, "harbor", "abc123")
    ).resolves.toBe(false);

    expect(info.mock.calls).toEqual([
      [
        "checkout cache hydrate",
        {
          key: "repos/harbor-abc123.tar.gz",
          scope: "harbor",
          commit: "abc123",
          result: "miss",
        },
      ],
    ]);
  });

  it("logs when a checkout snapshot is written", async () => {
    const client = makeMemoryGcsClient();
    const store = makeGcsCacheStore({ bucket: "b", client });
    const src = makeTmpDir();
    writeFileSync(join(src, "task.toml"), "name = 'a'\n");
    const info = captureInfo();

    await store.snapshotCheckout(src, "harbor", "deadbeefdeadbeef");

    expect(info.mock.calls).toEqual([
      [
        "checkout cache snapshot written",
        {
          key: "repos/harbor-deadbeefdead.tar.gz",
          scope: "harbor",
          commit: "deadbeefdeadbeef",
        },
      ],
    ]);
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

describe("DiskCacheStore", () => {
  const savedCacheDir = process.env.BENCH_DATASET_CACHE_DIR;
  const savedDisable = process.env.BENCH_DATASET_CACHE_DISABLE;
  const savedLogLevel = process.env.LOG_LEVEL;
  const savedOrEnv = process.env.OR_ENV;
  const tmpDirs: string[] = [];
  const infoSpies: { mockRestore: () => void }[] = [];

  afterEach(() => {
    if (savedCacheDir === undefined) {
      delete process.env.BENCH_DATASET_CACHE_DIR;
    } else {
      process.env.BENCH_DATASET_CACHE_DIR = savedCacheDir;
    }
    if (savedDisable === undefined) {
      delete process.env.BENCH_DATASET_CACHE_DISABLE;
    } else {
      process.env.BENCH_DATASET_CACHE_DISABLE = savedDisable;
    }
    if (savedLogLevel === undefined) {
      delete process.env.LOG_LEVEL;
    } else {
      process.env.LOG_LEVEL = savedLogLevel;
    }
    if (savedOrEnv === undefined) {
      delete process.env.OR_ENV;
    } else {
      process.env.OR_ENV = savedOrEnv;
    }
    for (const info of infoSpies.splice(0)) {
      info.mockRestore();
    }
    while (tmpDirs.length > 0) {
      const dir = tmpDirs.pop();
      if (dir !== undefined) {
        rmSync(dir, { recursive: true, force: true });
      }
    }
  });

  it("logs enabled disk JSON cache classifications", async () => {
    const root = mkdtempSync(join(tmpdir(), "disk-cache-test-"));
    tmpDirs.push(root);
    process.env.BENCH_DATASET_CACHE_DIR = root;
    process.env.BENCH_DATASET_CACHE_DISABLE = "0";
    process.env.OR_ENV = "development";
    process.env.LOG_LEVEL = "1";
    const info = spyOn(console, "info").mockImplementation(() => {});
    infoSpies.push(info);
    const store = makeDiskCacheStore();
    writeFileSync(join(root, "hit.json"), JSON.stringify({ ok: true }));

    await expect(store.readJson("hit.json")).resolves.toEqual({ ok: true });
    await expect(store.readJson("missing.json")).resolves.toBeUndefined();

    expect(info.mock.calls).toEqual([
      [
        "dataset cache read",
        { backend: "disk", key: "hit.json", result: "hit" },
      ],
      [
        "dataset cache read",
        { backend: "disk", key: "missing.json", result: "miss" },
      ],
    ]);
  });

  it("does not log disabled disk checkout no-ops", async () => {
    process.env.BENCH_DATASET_CACHE_DISABLE = "1";
    process.env.OR_ENV = "development";
    process.env.LOG_LEVEL = "1";
    const info = spyOn(console, "info").mockImplementation(() => {});
    infoSpies.push(info);
    const store = makeDiskCacheStore();

    await store.readJson("disabled.json");
    await store.tryHydrateCheckout("unused", "harbor", "abc123");
    await store.snapshotCheckout("unused", "harbor", "abc123");

    expect(info).not.toHaveBeenCalled();
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
