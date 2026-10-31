import type { ChildProcess } from "node:child_process";
import { spawn } from "node:child_process";
import { join } from "node:path";
import type { Readable, Writable } from "node:stream";
import { finished, pipeline } from "node:stream/promises";

import * as Result from "effect/Result";

import { unknownErrorToString } from "../internal/errors";
import { getGcsStorage } from "../internal/gcs";
import { iLog, wLog } from "../internal/log";
import type { AsyncResult } from "../internal/result";
import { tryPromiseResult } from "../internal/result";
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

export type DatasetCacheBackend = "disk" | "gcs";

export interface ReadJsonOptions {
  readonly maxAgeMs?: number;
  readonly now?: number;
}

type DatasetCacheReadResult = "hit" | "miss" | "stale" | "invalid";

function logDatasetCacheRead(
  backend: DatasetCacheBackend,
  key: string,
  result: DatasetCacheReadResult
): void {
  iLog("dataset cache read", { backend, key, result });
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

type TarOperation = "create" | "extract";

export type TarGzError =
  | {
      readonly kind: "tar_exited";
      readonly operation: TarOperation;
      readonly code: number | null;
      readonly stderr: string;
    }
  | {
      readonly kind: "spawn_failed";
      readonly operation: TarOperation;
      readonly message: string;
    }
  | {
      readonly kind: "stream_failed";
      readonly operation: TarOperation;
      readonly message: string;
    };

function describeTarGzError(error: TarGzError): string {
  switch (error.kind) {
    case "tar_exited": {
      return `tar ${error.operation} exited ${error.code}: ${error.stderr}`;
    }
    case "spawn_failed":
    case "stream_failed": {
      return error.message;
    }
    default: {
      return error satisfies never;
    }
  }
}

function waitForTar(
  proc: ChildProcess,
  operation: TarOperation
): AsyncResult<void, TarGzError> {
  let stderr = "";
  proc.stderr?.on("data", (d: Buffer) => (stderr += d.toString()));
  return new Promise((resolve) => {
    proc.once("error", (error) =>
      resolve(
        Result.fail({
          kind: "spawn_failed",
          operation,
          message: unknownErrorToString(error),
        })
      )
    );
    proc.once("close", (code) =>
      resolve(
        code === 0
          ? Result.succeed(undefined)
          : Result.fail({
              kind: "tar_exited",
              operation,
              code,
              stderr: stderr.slice(-1000),
            })
      )
    );
  });
}

async function streamStep(
  operation: TarOperation,
  step: () => Promise<void>
): AsyncResult<void, TarGzError> {
  const outcome = await tryPromiseResult(step);
  return Result.mapError(outcome, (error) => ({
    kind: "stream_failed",
    operation,
    message: unknownErrorToString(error),
  }));
}

async function abortTar(
  proc: ChildProcess,
  exit: AsyncResult<void, TarGzError>,
  streamFailure: Result.Failure<void, TarGzError>
): AsyncResult<void, TarGzError> {
  proc.kill();
  const exited = await exit;
  return Result.isFailure(exited) && exited.failure.kind === "spawn_failed"
    ? exited
    : streamFailure;
}

async function streamTarCreate(
  stdout: Readable,
  dest: Writable,
  exit: AsyncResult<void, TarGzError>
): AsyncResult<void, TarGzError> {
  const piped = await streamStep("create", () =>
    pipeline(stdout, dest, { end: false })
  );
  if (Result.isFailure(piped)) {
    return piped;
  }
  const exited = await exit;
  if (Result.isFailure(exited)) {
    return exited;
  }
  return streamStep("create", async () => {
    dest.end();
    await finished(dest);
  });
}

export async function pipeTarGzTo(
  dir: string,
  dest: Writable
): AsyncResult<void, TarGzError> {
  const proc = spawn("tar", ["-czf", "-", "-C", dir, "."], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  const exit = waitForTar(proc, "create");
  const streamed = await streamTarCreate(proc.stdout, dest, exit);
  if (Result.isFailure(streamed)) {
    dest.destroy(new Error(describeTarGzError(streamed.failure)));
    return abortTar(proc, exit, streamed);
  }
  return streamed;
}

export async function extractTarGzFrom(
  src: Readable,
  dir: string
): AsyncResult<void, TarGzError> {
  mkdirOwnerOnly(dir);
  const proc = spawn("tar", ["-xzf", "-", "-C", dir], {
    stdio: ["pipe", "ignore", "pipe"],
  });
  const exit = waitForTar(proc, "extract");
  const piped = await streamStep("extract", () => pipeline(src, proc.stdin));
  if (Result.isFailure(piped)) {
    return abortTar(proc, exit, piped);
  }
  return exit;
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
      const path = join(root, key);
      const value = readJsonCacheFile(path, opts);
      if (value !== undefined) {
        logDatasetCacheRead("disk", key, "hit");
        return value;
      }
      logDatasetCacheRead("disk", key, "miss");
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
          logDatasetCacheRead("gcs", full, "miss");
          return undefined;
        }
        const now = readOpts.now ?? Date.now();
        if (Math.max(0, now - updated) >= readOpts.maxAgeMs) {
          logDatasetCacheRead("gcs", full, "stale");
          return undefined;
        }
      }
      const buf = await client.downloadObject(full);
      if (buf === undefined) {
        logDatasetCacheRead("gcs", full, "miss");
        return undefined;
      }
      try {
        const value: unknown = JSON.parse(buf.toString("utf8"));
        logDatasetCacheRead("gcs", full, "hit");
        return value;
      } catch {
        logDatasetCacheRead("gcs", full, "invalid");
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
        wLog("GCS cache write failed", { key: full, error: String(error) });
      }
    },
    async tryHydrateCheckout(localDir, scope, commit) {
      const full = joinKey(prefix, checkoutSnapshotKey(scope, commit));
      const opened = await tryPromiseResult(() =>
        client.openObjectReadStream(full)
      );
      if (Result.isFailure(opened)) {
        wLog("GCS checkout hydrate failed", {
          key: full,
          error: unknownErrorToString(opened.failure),
        });
        return false;
      }
      if (opened.success === undefined) {
        iLog("checkout cache hydrate", {
          key: full,
          scope,
          commit,
          result: "miss",
        });
        return false;
      }
      const extracted = await extractTarGzFrom(opened.success, localDir);
      if (Result.isFailure(extracted)) {
        wLog("GCS checkout hydrate failed", {
          key: full,
          error: describeTarGzError(extracted.failure),
        });
        return false;
      }
      iLog("checkout cache hydrate", {
        key: full,
        scope,
        commit,
        result: "hit",
      });
      return true;
    },
    async snapshotCheckout(localDir, scope, commit) {
      const full = joinKey(prefix, checkoutSnapshotKey(scope, commit));
      const opened = await tryPromiseResult(async () =>
        client.openObjectWriteStream(full, "application/gzip")
      );
      if (Result.isFailure(opened)) {
        wLog("GCS checkout snapshot failed", {
          key: full,
          error: unknownErrorToString(opened.failure),
        });
        return;
      }
      const written = await pipeTarGzTo(localDir, opened.success);
      if (Result.isFailure(written)) {
        wLog("GCS checkout snapshot failed", {
          key: full,
          error: describeTarGzError(written.failure),
        });
        return;
      }
      iLog("checkout cache snapshot written", { key: full, scope, commit });
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
