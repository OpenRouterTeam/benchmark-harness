import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

import { Tag } from "effect/Context";
import type { Effect } from "effect/Effect";
import { fail, gen, tryPromise } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed } from "effect/Layer";

import type { AsyncEither } from "../../internal/either";
import { Either, tryPromiseEither } from "../../internal/either";
import { downloadFromGcs, uploadToGcs } from "../../internal/gcs";
import type { ValueOf } from "../../internal/guards";
import { wLog } from "../../internal/log";

export interface ArtifactStore {
  readonly get: (key: StageKey) => AsyncEither<string | undefined, string>;
  readonly put: (key: StageKey, json: string) => AsyncEither<void, string>;
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

export class ArtifactStoreService extends Tag(
  "@openrouter/bench-harness/benchmarks/draco/artifact-store/ArtifactStoreService"
)<ArtifactStoreService, ArtifactStore>() {}

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
  readonly get = (_key: StageKey): AsyncEither<string | undefined, string> =>
    Promise.resolve(Either.right(undefined));
  readonly put = (_key: StageKey, _json: string): AsyncEither<void, string> =>
    Promise.resolve(Either.right(undefined));
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
    return Either.isLeft(result) ? yield* fail(result.left) : result.right;
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
    if (Either.isLeft(result)) {
      yield* fail(result.left);
    }
  });
}

export class FsArtifactStore implements ArtifactStore {
  constructor(private readonly runDir: string) {}
  readonly get = async (
    key: StageKey
  ): AsyncEither<string | undefined, string> => {
    const r = await tryPromiseEither(() => readFile(this.path(key), "utf8"));
    if (Either.isLeft(r)) {
      const msg = String(r.left);
      return msg.includes("ENOENT")
        ? Either.right(undefined)
        : Either.left(`Failed to read ${key.stage}/${key.key}: ${msg}`);
    }
    return Either.right(r.right);
  };
  readonly put = async (
    key: StageKey,
    json: string
  ): AsyncEither<void, string> => {
    const r = await tryPromiseEither(async () => {
      const path = this.path(key);
      await mkdir(dirname(path), { recursive: true });
      await writeFile(path, json, "utf8");
    });
    return Either.isLeft(r)
      ? Either.left(
          `Failed to write ${key.stage}/${key.key}: ${String(r.left)}`
        )
      : Either.right(undefined);
  };
  private path(key: StageKey): string {
    return join(this.runDir, key.stage, `${key.key}.json`);
  }
}

export class GcsArtifactStore implements ArtifactStore {
  constructor(
    private readonly bucketName: string,
    private readonly prefix: string
  ) {}
  readonly get = async (
    key: StageKey
  ): AsyncEither<string | undefined, string> => {
    const objectPath = this.objectPath(key);
    const result = await downloadFromGcs({
      bucketName: this.bucketName,
      objectPath,
    });
    if (Either.isLeft(result)) {
      const msg = result.left.message ?? "unknown";
      if (msg.includes("No such object") || msg.includes("404")) {
        return Either.right(undefined);
      }
      wLog("GcsArtifactStore get degraded to miss", {
        bucket: this.bucketName,
        object_path: objectPath,
        error: msg,
      });
      return Either.right(undefined);
    }
    return Either.right(result.right.content.toString("utf8"));
  };
  readonly put = async (
    key: StageKey,
    json: string
  ): AsyncEither<void, string> => {
    const objectPath = this.objectPath(key);
    const result = await uploadToGcs({
      bucketName: this.bucketName,
      objectPath,
      content: json,
      contentType: "application/json",
    });
    if (Either.isLeft(result)) {
      return Either.left(
        `Failed to write ${key.stage}/${key.key} to GCS: ${result.left.message ?? "unknown"}`
      );
    }
    return Either.right(undefined);
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
