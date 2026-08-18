import { afterEach, describe, expect, it } from "bun:test";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

import {
  DATASET_CACHE_DIR_ENV,
  DATASET_CACHE_DISABLE_ENV,
  datasetCacheRoot,
  encodeCacheKeySegment,
  readJsonCacheFile,
  removeDirRecursive,
  writeJsonCacheFileAtomic,
} from "./local-cache";

const ENV_VARS: readonly string[] = [
  DATASET_CACHE_DIR_ENV,
  DATASET_CACHE_DISABLE_ENV,
];

function saveEnv(): Record<string, string | undefined> {
  return Object.fromEntries(ENV_VARS.map((name) => [name, process.env[name]]));
}

function restoreEnv(saved: Record<string, string | undefined>): void {
  for (const name of ENV_VARS) {
    const value = saved[name];
    if (value === undefined) {
      delete process.env[name];
    } else {
      process.env[name] = value;
    }
  }
}

describe("local-cache", () => {
  const saved = saveEnv();
  const tmpDirs: string[] = [];
  afterEach(() => {
    restoreEnv(saved);
    while (tmpDirs.length > 0) {
      const dir = tmpDirs.pop();
      if (dir !== undefined) {
        rmSync(dir, { recursive: true, force: true });
      }
    }
  });

  function makeTmpDir(): string {
    const dir = mkdtempSync(join(tmpdir(), "local-cache-test-"));
    tmpDirs.push(dir);
    return dir;
  }

  describe("datasetCacheRoot", () => {
    it("honors BENCH_DATASET_CACHE_DIR", () => {
      process.env[DATASET_CACHE_DIR_ENV] = "/tmp/custom-cache";
      delete process.env[DATASET_CACHE_DISABLE_ENV];
      expect(datasetCacheRoot()).toBe("/tmp/custom-cache");
    });
    it("defaults under the user's home cache dir", () => {
      delete process.env[DATASET_CACHE_DIR_ENV];
      delete process.env[DATASET_CACHE_DISABLE_ENV];
      expect(datasetCacheRoot()).toBe(
        join(homedir(), ".cache", "openrouter-bench-harness")
      );
    });
    it("returns undefined when disabled", () => {
      delete process.env[DATASET_CACHE_DIR_ENV];
      process.env[DATASET_CACHE_DISABLE_ENV] = "1";
      expect(datasetCacheRoot()).toBeUndefined();
      process.env[DATASET_CACHE_DISABLE_ENV] = "0";
      expect(datasetCacheRoot()).toBeDefined();
    });
  });

  describe("encodeCacheKeySegment", () => {
    it("encodes path separators and special chars", () => {
      expect(encodeCacheKeySegment("TIGER-Lab/MMLU-Pro")).toBe(
        "TIGER-Lab%2FMMLU-Pro"
      );
      expect(encodeCacheKeySegment("plain")).toBe("plain");
    });
  });

  describe("read/writeJsonCacheFile", () => {
    it("round-trips JSON and creates parent dirs", () => {
      const dir = makeTmpDir();
      const file = join(dir, "a", "b", "entry.json");
      writeJsonCacheFileAtomic(file, { hello: "world" });
      expect(readJsonCacheFile(file)).toEqual({ hello: "world" });
    });
    it("treats missing files and corrupt JSON as misses", () => {
      const dir = makeTmpDir();
      expect(readJsonCacheFile(join(dir, "nope.json"))).toBeUndefined();
      const bad = join(dir, "bad.json");
      writeJsonCacheFileAtomic(bad, { ok: true });
      writeFileSync(bad, "{not json");
      expect(readJsonCacheFile(bad)).toBeUndefined();
    });
    it("honors maxAgeMs staleness", () => {
      const dir = makeTmpDir();
      const file = join(dir, "entry.json");
      writeJsonCacheFileAtomic(file, [1, 2, 3]);
      const now = Date.now();
      expect(readJsonCacheFile(file, { maxAgeMs: 60_000, now })).toEqual([
        1, 2, 3,
      ]);
      expect(
        readJsonCacheFile(file, { maxAgeMs: 0, now: now + 60_000 })
      ).toBeUndefined();
    });
    it("does not leave tmp files behind", () => {
      const dir = makeTmpDir();
      const file = join(dir, "entry.json");
      writeJsonCacheFileAtomic(file, { ok: true });
      expect(existsSync(`${file}.tmp`)).toBe(false);
    });
  });

  describe("removeDirRecursive", () => {
    it("removes trees and ignores missing paths", () => {
      const dir = makeTmpDir();
      writeJsonCacheFileAtomic(join(dir, "sub", "x.json"), 1);
      removeDirRecursive(dir);
      expect(existsSync(dir)).toBe(false);
      removeDirRecursive(join(dir, "missing"));
    });
  });
});
