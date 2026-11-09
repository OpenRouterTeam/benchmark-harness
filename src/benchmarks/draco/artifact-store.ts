import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

import { Service } from "effect/Context";
import type { Effect } from "effect/Effect";
import { fail, gen, tryPromise } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed } from "effect/Layer";
import * as Result from "effect/Result";

import { downloadFromGcs, uploadToGcs } from "../../internal/gcs";
import type { ValueOf } from "../../internal/guards";
import { wLog } from "../../internal/log";
import type { AsyncResult } from "../../internal/result";
import { tryPromiseResult } from "../../internal/result";

export interface ArtifactStore {
  readonly get: (key: StageKey) => AsyncResult<string | undefined, string>;
  readonly put: (key: StageKey, json: string) => AsyncResult<void, string>;
}

export interface StageKey {
  readonly stage: StageDir;
  readonly key: string;
}

export const STAGE_DIRS = [
  "panelist",
  "synthesis",
  "judge",
  "generation",
] as const;

export type StageDir = ValueOf<typeof STAGE_DIRS>;

export class ArtifactStoreService extends Service<
  ArtifactStoreService,
  ArtifactStore
>()(
  "@openrouter/bench-harness/benchmarks/draco/artifact-store/ArtifactStoreService"
) {}

export function makeArtifactStoreLayer(
  store: ArtifactStore
): Layer<ArtifactStoreService> {
  return succeed(ArtifactStoreService, ArtifactStoreService.of(store));
}

export function makeFsArtifactStoreLayer(
  runDir: string
): Layer<ArtifactStoreService> {
  return makeArtifactStoreLayer(new FsArtifactStore(runDir));
}

export class NoopArtifactStore implements ArtifactStore {
  readonly get = (_key: StageKey): AsyncResult<string | undefined, string> =>
    Promise.resolve(Result.succeed(undefined));
  readonly put = (_key: StageKey, _json: string): AsyncResult<void, string> =>
    Promise.resolve(Result.succeed(undefined));
}

export function makeNoopArtifactStoreLayer(): Layer<ArtifactStoreService> {
  return makeArtifactStoreLayer(new NoopArtifactStore());
}

export function stageGet(
  key: StageKey
): Effect<string | undefined, string, ArtifactStoreService> {
  return gen(function* () {
    const store = yield* ArtifactStoreService;
    const result = yield* tryPromise({
      try: () => store.get(key),
      catch: (e) => `ArtifactStore get failed: ${String(e)}`,
    });
    return Result.isFailure(result)
      ? yield* fail(result.failure)
      : result.success;
  });
}

export function stagePut(
  key: StageKey,
  json: string
): Effect<void, string, ArtifactStoreService> {
  return gen(function* () {
    const store = yield* ArtifactStoreService;
    const result = yield* tryPromise({
      try: () => store.put(key, json),
      catch: (e) => `ArtifactStore put failed: ${String(e)}`,
    });
    if (Result.isFailure(result)) {
      yield* fail(result.failure);
    }
  });
}

export class FsArtifactStore implements ArtifactStore {
  private readonly runDir: string;
  constructor(runDir: string) {
    this.runDir = runDir;
  }
  readonly get = async (
    key: StageKey
  ): AsyncResult<string | undefined, string> => {
    const r = await tryPromiseResult(() => readFile(this.path(key), "utf8"));
    if (Result.isFailure(r)) {
      const msg = String(r.failure);
      return msg.includes("ENOENT")
        ? Result.succeed(undefined)
        : Result.fail(`Failed to read ${key.stage}/${key.key}: ${msg}`);
    }
    return Result.succeed(r.success);
  };
  readonly put = async (
    key: StageKey,
    json: string
  ): AsyncResult<void, string> => {
    const r = await tryPromiseResult(async () => {
      const path = this.path(key);
      await mkdir(dirname(path), { recursive: true });
      await writeFile(path, json, "utf8");
    });
    return Result.isFailure(r)
      ? Result.fail(
          `Failed to write ${key.stage}/${key.key}: ${String(r.failure)}`
        )
      : Result.succeed(undefined);
  };
  private path(key: StageKey): string {
    return join(this.runDir, key.stage, `${key.key}.json`);
  }
}

export class GcsArtifactStore implements ArtifactStore {
  private readonly bucketName: string;
  private readonly prefix: string;
  constructor(bucketName: string, prefix: string) {
    this.bucketName = bucketName;
    this.prefix = prefix;
  }
  readonly get = async (
    key: StageKey
  ): AsyncResult<string | undefined, string> => {
    const objectPath = this.objectPath(key);
    const result = await downloadFromGcs({
      bucketName: this.bucketName,
      objectPath,
    });
    if (Result.isFailure(result)) {
      const msg = result.failure.message ?? "unknown";
      if (msg.includes("No such object") || msg.includes("404")) {
        return Result.succeed(undefined);
      }
      wLog("GcsArtifactStore get degraded to miss", {
        bucket: this.bucketName,
        object_path: objectPath,
        error: msg,
      });
      return Result.succeed(undefined);
    }
    return Result.succeed(result.success.content.toString("utf8"));
  };
  readonly put = async (
    key: StageKey,
    json: string
  ): AsyncResult<void, string> => {
    const objectPath = this.objectPath(key);
    const result = await uploadToGcs({
      bucketName: this.bucketName,
      objectPath,
      content: json,
      contentType: "application/json",
    });
    if (Result.isFailure(result)) {
      return Result.fail(
        `Failed to write ${key.stage}/${key.key} to GCS: ${result.failure.message ?? "unknown"}`
      );
    }
    return Result.succeed(undefined);
  };
  objectPath(key: StageKey): string {
    return `${this.prefix}/${key.stage}/${key.key}.json`;
  }
}

export function makeGcsArtifactStoreLayer(
  bucketName: string,
  prefix: string
): Layer<ArtifactStoreService> {
  return makeArtifactStoreLayer(new GcsArtifactStore(bucketName, prefix));
}
