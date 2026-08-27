import { spawn } from "node:child_process";
import { join } from "node:path";
import type { Readable, Writable } from "node:stream";
import { finished, pipeline } from "node:stream/promises";

import { getGcsStorage } from "../internal/gcs";
import { wLog } from "../internal/log";
import {
  datasetCacheDisabled,
  datasetCacheRoot,
  mkdirOwnerOnly,
  readJsonCacheFile,
  writeJsonCacheFileAtomic,
} from "./local-cache";

export const CACHE_BACKEND_ENV = "BENCH_DATASET_CACHE_BACKEND";
export const GCS_BUCKET_ENV = "BENCH_GCS_BUCKET";
export const GCS_PREFIX_ENV = "BENCH_GCS_PREFIX";

export type DatasetCacheBackend = "disk" | "gcs";

export interface ReadJsonOptions {
  readonly maxAgeMs?: number;
  readonly now?: number;
}

export interface CacheStore {
  readonly backend: DatasetCacheBackend;
  readonly enabled: boolean;
  readonly readJson: (
    key: string,
    opts?: ReadJsonOptions
  ) => Promise<unknown | undefined>;
  readonly writeJson: (key: string, value: unknown) => Promise<void>;
  readonly tryHydrateCheckout: (
    localDir: string,
    scope: string,
    commit: string
  ) => Promise<boolean>;
  readonly snapshotCheckout: (
    localDir: string,
    scope: string,
    commit: string
  ) => Promise<void>;
}

async function waitForTar(
  proc: ReturnType<typeof spawn>,
  operation: string
): Promise<void> {
  let stderr = "";
  proc.stderr?.on("data", (d: Buffer) => (stderr += d.toString()));
  await new Promise<void>((resolve, reject) => {
    proc.once("error", reject);
    proc.once("close", (code) =>
      code === 0
        ? resolve()
        : reject(
            new Error(`tar ${operation} exited ${code}: ${stderr.slice(-1000)}`)
          )
    );
  });
}

export async function pipeTarGzTo(dir: string, dest: Writable): Promise<void> {
  const proc = spawn("tar", ["-czf", "-", "-C", dir, "."], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (proc.stdout === null) {
    throw new Error("tar create did not provide stdout");
  }
  const exit = waitForTar(proc, "create");
  try {
    await pipeline(proc.stdout, dest, { end: false });
    await exit;
    dest.end();
    await finished(dest);
  } catch (error) {
    const destinationError =
      error instanceof Error ? error : new Error(String(error));
    dest.destroy(destinationError);
    proc.kill();
    await exit.catch(() => undefined);
    throw error;
  }
}

export async function extractTarGzFrom(
  src: Readable,
  dir: string
): Promise<void> {
  mkdirOwnerOnly(dir);
  const proc = spawn("tar", ["-xzf", "-", "-C", dir], {
    stdio: ["pipe", "ignore", "pipe"],
  });
  if (proc.stdin === null) {
    throw new Error("tar extract did not provide stdin");
  }
  const exit = waitForTar(proc, "extract");
  try {
    await pipeline(src, proc.stdin);
  } catch (error) {
    proc.kill();
    await exit.catch(() => undefined);
    throw error;
  }
  await exit;
}

export function makeDiskCacheStore(): CacheStore {
  return {
    backend: "disk",
    get enabled(): boolean {
      return datasetCacheRoot() !== undefined;
    },
    async readJson(key, opts) {
      const root = datasetCacheRoot();
      if (root === undefined) {
        return undefined;
      }
      return readJsonCacheFile(join(root, key), opts);
    },
    async writeJson(key, value) {
      const root = datasetCacheRoot();
      if (root === undefined) {
        return;
      }
      writeJsonCacheFileAtomic(join(root, key), value);
    },
    async tryHydrateCheckout() {
      return false;
    },
    async snapshotCheckout() {},
  };
}

export interface GcsObjectClient {
  readonly downloadObject: (key: string) => Promise<Buffer | undefined>;
  readonly uploadObject: (
    key: string,
    content: Buffer,
    contentType?: string
  ) => Promise<void>;
  readonly openObjectReadStream: (key: string) => Promise<Readable | undefined>;
  readonly openObjectWriteStream: (
    key: string,
    contentType?: string
  ) => Writable;
  readonly objectUpdatedMs: (key: string) => Promise<number | undefined>;
}

export interface GcsCacheStoreOptions {
  readonly bucket: string;
  readonly prefix?: string;
  readonly client?: GcsObjectClient;
}

function normalizePrefix(prefix: string | undefined): string {
  if (prefix === undefined || prefix.length === 0) {
    return "";
  }
  return prefix.replace(/^\/+/, "").replace(/\/+$/, "");
}

function joinKey(prefix: string, key: string): string {
  if (prefix.length === 0) {
    return key;
  }
  return `${prefix}/${key}`;
}

function checkoutSnapshotKey(scope: string, commit: string): string {
  return `repos/${scope}-${commit.slice(0, 12)}.tar.gz`;
}

export function makeDefaultGcsClient(bucket: string): GcsObjectClient {
  const bucketRef = getGcsStorage().bucket(bucket);
  return {
    async downloadObject(key) {
      try {
        const [buf] = await bucketRef.file(key).download();
        return buf;
      } catch (error) {
        wLog("GCS cache download miss/failed", {
          key,
          error: String(error),
        });
        return undefined;
      }
    },
    async uploadObject(key, content, contentType = "application/octet-stream") {
      try {
        await bucketRef.file(key).save(content, { contentType });
      } catch (error) {
        wLog("GCS cache upload failed", { key, error: String(error) });
      }
    },
    async openObjectReadStream(key) {
      const file = bucketRef.file(key);
      const [exists] = await file.exists();
      return exists ? file.createReadStream() : undefined;
    },
    openObjectWriteStream(key, contentType = "application/octet-stream") {
      return bucketRef.file(key).createWriteStream({
        contentType,
        resumable: false,
      });
    },
    async objectUpdatedMs(key) {
      try {
        const [meta] = await bucketRef.file(key).getMetadata();
        const updated = meta?.updated;
        return typeof updated === "string" ? Date.parse(updated) : undefined;
      } catch {
        return undefined;
      }
    },
  };
}

export function makeGcsCacheStore(opts: GcsCacheStoreOptions): CacheStore {
  const prefix = normalizePrefix(opts.prefix);
  const client: GcsObjectClient =
    opts.client ?? makeDefaultGcsClient(opts.bucket);
  return {
    backend: "gcs",
    get enabled(): boolean {
      return !datasetCacheDisabled();
    },
    async readJson(key, readOpts) {
      const full = joinKey(prefix, key);
      if (readOpts?.maxAgeMs !== undefined) {
        const updated = await client.objectUpdatedMs(full);
        if (updated === undefined) {
          return undefined;
        }
        const now = readOpts.now ?? Date.now();
        if (Math.max(0, now - updated) >= readOpts.maxAgeMs) {
          return undefined;
        }
      }
      const buf = await client.downloadObject(full);
      if (buf === undefined) {
        return undefined;
      }
      try {
        return JSON.parse(buf.toString("utf8"));
      } catch {
        return undefined;
      }
    },
    async writeJson(key, value) {
      const full = joinKey(prefix, key);
      try {
        await client.uploadObject(
          full,
          Buffer.from(JSON.stringify(value), "utf8"),
          "application/json"
        );
      } catch (error) {
        wLog("GCS cache write failed", { key: full, error: String(error) });
      }
    },
    async tryHydrateCheckout(localDir, scope, commit) {
      const full = joinKey(prefix, checkoutSnapshotKey(scope, commit));
      try {
        const src = await client.openObjectReadStream(full);
        if (src === undefined) {
          return false;
        }
        await extractTarGzFrom(src, localDir);
        return true;
      } catch (error) {
        wLog("GCS checkout hydrate failed", {
          key: full,
          error: String(error),
        });
        return false;
      }
    },
    async snapshotCheckout(localDir, scope, commit) {
      const full = joinKey(prefix, checkoutSnapshotKey(scope, commit));
      try {
        const dest = client.openObjectWriteStream(full, "application/gzip");
        await pipeTarGzTo(localDir, dest);
      } catch (error) {
        wLog("GCS checkout snapshot failed", {
          key: full,
          error: String(error),
        });
      }
    },
  };
}

export function resolveCacheStore(): CacheStore {
  const backend = readBackendEnv();
  if (backend === "gcs") {
    const bucket = readEnv(GCS_BUCKET_ENV);
    if (bucket === undefined || bucket.length === 0) {
      wLog(
        `BENCH_DATASET_CACHE_BACKEND=gcs but ${GCS_BUCKET_ENV} is unset; falling back to disk`,
        {}
      );
      return makeDiskCacheStore();
    }
    return makeGcsCacheStore({
      bucket,
      prefix: readEnv(GCS_PREFIX_ENV),
    });
  }
  return makeDiskCacheStore();
}

function readBackendEnv(): DatasetCacheBackend {
  const raw = readEnv(CACHE_BACKEND_ENV);
  if (raw === "gcs") {
    return "gcs";
  }
  return "disk";
}

function readEnv(name: string): string | undefined {
  const value = process.env[name];
  return value !== undefined && value.length > 0 ? value : undefined;
}
