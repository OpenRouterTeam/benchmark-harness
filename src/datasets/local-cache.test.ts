import { afterEach, describe, expect, it } from "bun:test";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

import {
  CHECKOUT_COMPLETE_MARKER,
  DATASET_CACHE_DIR_ENV,
  DATASET_CACHE_DISABLE_ENV,
  STALE_STAGING_MAX_AGE_MS,
  datasetCacheRoot,
  encodeCacheKeySegment,
  hasCheckoutCompleteMarker,
  mkdirOwnerOnly,
  publishStagedCheckout,
  readJsonCacheFile,
  removeDirRecursive,
  restrictPermissionsRecursive,
  sweepStaleStagingDirs,
  writeCheckoutCompleteMarker,
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
      process.env[DATASET_CACHE_DISABLE_ENV] = "0";
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
    it("defaults to disabled under bun test without an explicit cache dir", () => {
      delete process.env[DATASET_CACHE_DIR_ENV];
      delete process.env[DATASET_CACHE_DISABLE_ENV];
      expect(datasetCacheRoot()).toBeUndefined();
    });
    it("stays enabled under bun test when a cache dir is set", () => {
      const dir = makeTmpDir();
      process.env[DATASET_CACHE_DIR_ENV] = dir;
      delete process.env[DATASET_CACHE_DISABLE_ENV];
      expect(datasetCacheRoot()).toBe(dir);
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
      expect(statSync(file).mode & 0o777).toBe(0o600);
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

  describe("mkdirOwnerOnly", () => {
    it("creates every new level with owner-only permissions and leaves existing dirs alone", () => {
      const base = makeTmpDir();
      chmodSync(base, 0o755);
      const path = join(base, "a", "b", "c");
      mkdirOwnerOnly(path);
      expect(statSync(join(base, "a")).mode & 0o777).toBe(0o700);
      expect(statSync(join(base, "a", "b")).mode & 0o777).toBe(0o700);
      expect(statSync(path).mode & 0o777).toBe(0o700);
      expect(statSync(base).mode & 0o777).toBe(0o755);
    });
    it("tightens a pre-existing target dir without touching its ancestors", () => {
      const base = makeTmpDir();
      chmodSync(base, 0o755);
      const target = join(base, "hf");
      mkdirSync(target, { mode: 0o755 });
      mkdirOwnerOnly(target);
      expect(statSync(target).mode & 0o777).toBe(0o700);
      expect(statSync(base).mode & 0o777).toBe(0o755);
    });
  });

  describe("sweepStaleStagingDirs", () => {
    it("removes only staging dirs older than the max age", () => {
      const dir = makeTmpDir();
      const stale = join(dir, ".old-label-abc.staging-zzzzzz");
      const fresh = join(dir, ".new-label-abc.staging-aaaaaa");
      const checkout = join(dir, "some-label-abc1234567890");
      for (const d of [stale, fresh, checkout]) {
        mkdirSync(d);
      }
      const old = new Date(Date.now() - STALE_STAGING_MAX_AGE_MS - 1000);
      utimesSync(stale, old, old);
      sweepStaleStagingDirs(dir);
      expect(existsSync(stale)).toBe(false);
      expect(existsSync(fresh)).toBe(true);
      expect(existsSync(checkout)).toBe(true);
    });
    it("ignores missing dirs", () => {
      sweepStaleStagingDirs(join(makeTmpDir(), "does-not-exist"));
    });
  });

  describe("checkout completion marker", () => {
    it("round-trips the marker and defaults to absent", () => {
      const dir = makeTmpDir();
      expect(hasCheckoutCompleteMarker(dir)).toBe(false);
      writeCheckoutCompleteMarker(dir);
      expect(hasCheckoutCompleteMarker(dir)).toBe(true);
      expect(existsSync(join(dir, CHECKOUT_COMPLETE_MARKER))).toBe(true);
      expect(statSync(join(dir, CHECKOUT_COMPLETE_MARKER)).mode & 0o777).toBe(
        0o600
      );
    });
  });

  describe("restrictPermissionsRecursive", () => {
    it("strips group and other permissions while keeping owner exec bits", () => {
      const dir = makeTmpDir();
      const nested = join(dir, "sub", "deep");
      mkdirSync(nested, { recursive: true });
      writeFileSync(join(nested, "plain.txt"), "x\n");
      writeFileSync(join(nested, "run.sh"), "x\n", { mode: 0o755 });
      chmodSync(join(dir, "sub"), 0o755);
      restrictPermissionsRecursive(dir);
      expect(statSync(dir).mode & 0o777).toBe(0o700);
      expect(statSync(join(dir, "sub")).mode & 0o777).toBe(0o700);
      expect(statSync(join(nested, "plain.txt")).mode & 0o777).toBe(0o600);
      expect(statSync(join(nested, "run.sh")).mode & 0o777).toBe(0o700);
    });
  });

  describe("publishStagedCheckout", () => {
    it("renames the staging dir into a free shared path", () => {
      const dir = makeTmpDir();
      const staging = join(dir, "staging");
      const shared = join(dir, "shared");
      mkdirSync(staging);
      writeFileSync(join(staging, "task.toml"), "task\n");
      const root = publishStagedCheckout(staging, shared, () => true);
      expect(root).toBe(shared);
      expect(existsSync(join(shared, "task.toml"))).toBe(true);
      expect(existsSync(staging)).toBe(false);
    });
    it("keeps a concurrently published complete checkout and discards staging", () => {
      const dir = makeTmpDir();
      const staging = join(dir, "staging");
      const shared = join(dir, "shared");
      mkdirSync(staging);
      mkdirSync(shared);
      writeCheckoutCompleteMarker(shared);
      writeFileSync(join(shared, "task.toml"), "winner\n");
      const root = publishStagedCheckout(staging, shared, () => true);
      expect(root).toBe(shared);
      expect(readFileSync(join(shared, "task.toml"), "utf8")).toBe("winner\n");
      expect(existsSync(staging)).toBe(false);
    });
    it("replaces a marker-less leftover shared dir", () => {
      const dir = makeTmpDir();
      const staging = join(dir, "staging");
      const shared = join(dir, "shared");
      mkdirSync(staging);
      writeCheckoutCompleteMarker(staging);
      writeFileSync(join(staging, "task.toml"), "fresh\n");
      mkdirSync(shared);
      writeFileSync(join(shared, "partial.txt"), "interrupted run\n");
      const root = publishStagedCheckout(staging, shared, () => true);
      expect(root).toBe(shared);
      expect(readFileSync(join(shared, "task.toml"), "utf8")).toBe("fresh\n");
      expect(existsSync(join(shared, "partial.txt"))).toBe(false);
      expect(existsSync(staging)).toBe(false);
    });
    it("never deletes a marked shared dir that is missing its tasks", () => {
      const dir = makeTmpDir();
      const staging = join(dir, "staging");
      const shared = join(dir, "shared");
      mkdirSync(staging);
      mkdirSync(shared);
      writeCheckoutCompleteMarker(shared);
      writeFileSync(join(shared, "partial.txt"), "in use elsewhere\n");
      expect(() => publishStagedCheckout(staging, shared, () => false)).toThrow(
        /incomplete; remove it manually/
      );
      expect(readFileSync(join(shared, "partial.txt"), "utf8")).toBe(
        "in use elsewhere\n"
      );
      expect(existsSync(staging)).toBe(false);
    });
  });
});
