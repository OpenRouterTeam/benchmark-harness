import { spawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import type { FileHandle } from "node:fs/promises";
import { open, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Readable, Writable } from "node:stream";
import { finished, pipeline } from "node:stream/promises";

import { AwsClient } from "aws4fetch";

import { getGcsStorage } from "../internal/gcs";
import { iLog, wLog } from "../internal/log";
import {
  datasetCacheDisabled,
  datasetCacheRoot,
  mkdirOwnerOnly,
  readJsonCacheFile,
  writeJsonCacheFileAtomic,
} from "./local-cache";

let writeFailures = 0;

export function datasetCacheWriteFailures(): number {
  return writeFailures;
}

export const CACHE_BACKEND_ENV = "BENCH_DATASET_CACHE_BACKEND";
export const GCS_BUCKET_ENV = "BENCH_GCS_BUCKET";
export const GCS_PREFIX_ENV = "BENCH_GCS_PREFIX";
export const S3_ENDPOINT_ENV = "BENCH_S3_ENDPOINT";
export const S3_BUCKET_ENV = "BENCH_S3_BUCKET";
export const S3_PREFIX_ENV = "BENCH_S3_PREFIX";
export const S3_ACCESS_KEY_ID_ENV = "BENCH_S3_ACCESS_KEY_ID";
export const S3_SECRET_ACCESS_KEY_ENV = "BENCH_S3_SECRET_ACCESS_KEY";

export type DatasetCacheBackend = "disk" | "gcs" | "s3";

export interface ReadJsonOptions {
  readonly maxAgeMs?: number;
  readonly now?: number;
}

type DatasetCacheReadResult = "hit" | "miss" | "stale" | "invalid";

function logDatasetCacheRead(
  backend: DatasetCacheBackend,
  key: string,
  result: DatasetCacheReadResult,
  startedMs: number
): void {
  iLog("dataset cache read", {
    backend,
    key,
    result,
    duration_ms: Date.now() - startedMs,
  });
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
      const startedMs = Date.now();
      const root = datasetCacheRoot();
      if (root === undefined) {
        return undefined;
      }
      const path = join(root, key);
      const value = readJsonCacheFile(path, opts);
      if (value !== undefined) {
        logDatasetCacheRead("disk", key, "hit", startedMs);
        return value;
      }
      logDatasetCacheRead("disk", key, "miss", startedMs);
      return undefined;
    },
    async writeJson(key, value) {
      const root = datasetCacheRoot();
      if (root === undefined) {
        return;
      }
      const written = writeJsonCacheFileAtomic(join(root, key), value);
      if (!written) {
        writeFailures += 1;
      }
    },
    async tryHydrateCheckout() {
      return false;
    },
    async snapshotCheckout() {},
  };
}

export interface ObjectClient {
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

export type GcsObjectClient = ObjectClient;

export interface GcsCacheStoreOptions {
  readonly bucket: string;
  readonly prefix?: string;
  readonly client?: ObjectClient;
}

export interface S3CacheStoreOptions {
  readonly endpoint: string;
  readonly bucket: string;
  readonly accessKeyId: string;
  readonly secretAccessKey: string;
  readonly prefix?: string;
  readonly client?: ObjectClient;
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
        await bucketRef.file(key).save(content, {
          contentType,
          resumable: false,
        });
      } catch (error) {
        writeFailures += 1;
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

interface BunS3File {
  readonly arrayBuffer: () => Promise<ArrayBuffer>;
  readonly stream: () => ReadableStream<Uint8Array>;
  readonly exists: () => Promise<boolean>;
  readonly stat: () => Promise<{ readonly lastModified: Date }>;
}

interface BunS3Client {
  readonly file: (key: string) => BunS3File;
  readonly write: (
    key: string,
    body: Uint8Array | BunS3File,
    options: { readonly type: string }
  ) => Promise<number>;
}

interface BunS3Runtime {
  readonly S3Client: new (options: {
    readonly endpoint: string;
    readonly bucket: string;
    readonly accessKeyId: string;
    readonly secretAccessKey: string;
    readonly region: string;
    readonly retry: number;
  }) => BunS3Client;
  readonly file: (path: string) => BunS3File;
}

const SESSION_TOKEN_ENVS = ["AWS_SESSION_TOKEN", "S3_SESSION_TOKEN"] as const;

function s3ErrorCode(error: unknown): string | undefined {
  if (typeof error !== "object" || error === null || !("code" in error)) {
    return undefined;
  }
  return typeof error.code === "string" ? error.code : undefined;
}

function isMissingObject(error: unknown): boolean {
  const code = s3ErrorCode(error);
  return code === "NoSuchKey" || code === "NoSuchBucket";
}

function bunS3Runtime(): BunS3Runtime {
  const bun = (globalThis as { readonly Bun?: BunS3Runtime }).Bun;
  if (bun === undefined) {
    throw new Error("The s3 dataset cache backend requires the Bun runtime");
  }
  const tokenEnv = SESSION_TOKEN_ENVS.find(
    (name) => process.env[name] !== undefined
  );
  if (tokenEnv !== undefined) {
    throw new Error(
      `The s3 dataset cache backend uses static keys; unset ${tokenEnv}, which Bun's S3 client would send with them`
    );
  }
  return bun;
}

export function makeDefaultS3Client(opts: {
  readonly endpoint: string;
  readonly bucket: string;
  readonly accessKeyId: string;
  readonly secretAccessKey: string;
}): ObjectClient {
  const bun = bunS3Runtime();
  const client = new bun.S3Client({
    endpoint: opts.endpoint,
    bucket: opts.bucket,
    accessKeyId: opts.accessKeyId,
    secretAccessKey: opts.secretAccessKey,
    region: "us-east-1",
    retry: 0,
  });
  const aws = new AwsClient({
    accessKeyId: opts.accessKeyId,
    secretAccessKey: opts.secretAccessKey,
    service: "s3",
    region: "us-east-1",
    retries: 0,
  });
  const bucketUrl = `${opts.endpoint.replace(/\/+$/, "")}/${encodeURIComponent(opts.bucket)}`;

  async function ensureBucket(): Promise<void> {
    const created = await aws.fetch(bucketUrl, { method: "PUT" });
    await created.body?.cancel();
    if (!created.ok && created.status !== 409) {
      throw new Error(`S3 create bucket failed: HTTP ${created.status}`);
    }
  }

  async function write(
    key: string,
    body: Uint8Array | BunS3File,
    contentType: string
  ): Promise<void> {
    const put = (): Promise<number> =>
      client.write(key, body, { type: contentType });
    try {
      await put();
    } catch (error) {
      const code = s3ErrorCode(error);
      if (code !== "NoSuchBucket" && code !== "UnknownError") {
        throw error;
      }
      await ensureBucket();
      await put();
    }
  }

  return {
    async downloadObject(key) {
      try {
        return Buffer.from(await client.file(key).arrayBuffer());
      } catch (error) {
        if (!isMissingObject(error)) {
          wLog("S3 cache download failed", { key, error: String(error) });
        }
        return undefined;
      }
    },
    async uploadObject(key, content, contentType = "application/octet-stream") {
      try {
        await write(key, content, contentType);
      } catch (error) {
        writeFailures += 1;
        wLog("S3 cache upload failed", { key, error: String(error) });
      }
    },
    async openObjectReadStream(key) {
      const file = client.file(key);
      return (await file.exists()) ? Readable.from(file.stream()) : undefined;
    },
    openObjectWriteStream(key, contentType = "application/octet-stream") {
      const spoolDir = mkdtempSync(join(tmpdir(), "bench-s3-upload-"));
      const spoolPath = join(spoolDir, "object");
      let spool: FileHandle | undefined;
      const cleanup = async (): Promise<void> => {
        await spool?.close().catch(() => undefined);
        spool = undefined;
        await rm(spoolDir, { recursive: true, force: true });
      };
      const cleanupOnFailure = async (
        step: () => Promise<void>
      ): Promise<void> => {
        try {
          await step();
        } catch (error) {
          await cleanup();
          throw error;
        }
      };
      return Writable.fromWeb(
        new WritableStream<Uint8Array>({
          start: () =>
            cleanupOnFailure(async () => {
              spool = await open(spoolPath, "w");
            }),
          write: (chunk) =>
            cleanupOnFailure(async () => {
              await spool?.writeFile(chunk);
            }),
          async close() {
            try {
              await spool?.close();
              spool = undefined;
              await write(key, bun.file(spoolPath), contentType);
            } finally {
              await cleanup();
            }
          },
          abort: cleanup,
        })
      );
    },
    async objectUpdatedMs(key) {
      try {
        const { lastModified } = await client.file(key).stat();
        const updated = lastModified.getTime();
        return Number.isFinite(updated) ? updated : undefined;
      } catch (error) {
        if (!isMissingObject(error)) {
          wLog("S3 cache head failed", { key, error: String(error) });
        }
        return undefined;
      }
    },
  };
}

export function makeGcsCacheStore(opts: GcsCacheStoreOptions): CacheStore {
  return makeObjectCacheStore(
    "gcs",
    opts.prefix,
    opts.client ?? makeDefaultGcsClient(opts.bucket)
  );
}

export function makeS3CacheStore(opts: S3CacheStoreOptions): CacheStore {
  return makeObjectCacheStore(
    "s3",
    opts.prefix,
    opts.client ?? makeDefaultS3Client(opts)
  );
}

function makeObjectCacheStore(
  backend: "gcs" | "s3",
  rawPrefix: string | undefined,
  client: ObjectClient
): CacheStore {
  const prefix = normalizePrefix(rawPrefix);
  const label = backend.toUpperCase();
  return {
    backend,
    get enabled(): boolean {
      return !datasetCacheDisabled();
    },
    async readJson(key, readOpts) {
      const startedMs = Date.now();
      const full = joinKey(prefix, key);
      if (readOpts?.maxAgeMs !== undefined) {
        const updated = await client.objectUpdatedMs(full);
        if (updated === undefined) {
          logDatasetCacheRead(backend, full, "miss", startedMs);
          return undefined;
        }
        const now = readOpts.now ?? Date.now();
        if (Math.max(0, now - updated) >= readOpts.maxAgeMs) {
          logDatasetCacheRead(backend, full, "stale", startedMs);
          return undefined;
        }
      }
      const buf = await client.downloadObject(full);
      if (buf === undefined) {
        logDatasetCacheRead(backend, full, "miss", startedMs);
        return undefined;
      }
      try {
        const value: unknown = JSON.parse(buf.toString("utf8"));
        logDatasetCacheRead(backend, full, "hit", startedMs);
        return value;
      } catch {
        logDatasetCacheRead(backend, full, "invalid", startedMs);
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
        writeFailures += 1;
        wLog(`${label} cache write failed`, {
          key: full,
          error: String(error),
        });
      }
    },
    async tryHydrateCheckout(localDir, scope, commit) {
      const full = joinKey(prefix, checkoutSnapshotKey(scope, commit));
      try {
        const src = await client.openObjectReadStream(full);
        if (src === undefined) {
          iLog("checkout cache hydrate", {
            key: full,
            scope,
            commit,
            result: "miss",
          });
          return false;
        }
        await extractTarGzFrom(src, localDir);
        iLog("checkout cache hydrate", {
          key: full,
          scope,
          commit,
          result: "hit",
        });
        return true;
      } catch (error) {
        wLog(`${label} checkout hydrate failed`, {
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
        iLog("checkout cache snapshot written", { key: full, scope, commit });
      } catch (error) {
        wLog(`${label} checkout snapshot failed`, {
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
  if (backend === "s3") {
    return makeS3CacheStore({
      endpoint: requireEnv(S3_ENDPOINT_ENV),
      bucket: requireEnv(S3_BUCKET_ENV),
      accessKeyId: requireEnv(S3_ACCESS_KEY_ID_ENV),
      secretAccessKey: requireEnv(S3_SECRET_ACCESS_KEY_ENV),
      prefix: readEnv(S3_PREFIX_ENV),
    });
  }
  return makeDiskCacheStore();
}

function readBackendEnv(): DatasetCacheBackend {
  const raw = readEnv(CACHE_BACKEND_ENV);
  if (raw === "gcs" || raw === "s3") {
    return raw;
  }
  return "disk";
}

function readEnv(name: string): string | undefined {
  const value = process.env[name];
  return value !== undefined && value.length > 0 ? value : undefined;
}

function requireEnv(name: string): string {
  const value = readEnv(name);
  if (value === undefined) {
    throw new Error(`${CACHE_BACKEND_ENV}=s3 requires ${name}`);
  }
  return value;
}
