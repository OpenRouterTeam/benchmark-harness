import {
  chmodSync,
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, join } from "node:path";

import { option, string } from "effect/Config";
import { runSync } from "effect/Effect";
import { getOrNull } from "effect/Option";

import { wLog } from "../internal/log";

export const DATASET_CACHE_DIR_ENV = "BENCH_DATASET_CACHE_DIR";
export const DATASET_CACHE_DISABLE_ENV = "BENCH_DATASET_CACHE_DISABLE";
export const CHECKOUT_COMPLETE_MARKER = ".bench-checkout-complete";

export const STALE_STAGING_MAX_AGE_MS = 24 * 60 * 60 * 1e3;

export function readEnvOptional(name: string): string | undefined {
  const value = getOrNull(runSync(string(name).pipe(option)));
  return value !== null && value.length > 0 ? value : undefined;
}

export function datasetCacheDisabled(): boolean {
  const raw = readEnvOptional(DATASET_CACHE_DISABLE_ENV);
  if (raw !== undefined) {
    return raw !== "0" && raw !== "false";
  }
  return (
    runningUnderTest() && readEnvOptional(DATASET_CACHE_DIR_ENV) === undefined
  );
}

function runningUnderTest(): boolean {
  return process.env.BUN_TEST === "1" || process.env.NODE_ENV === "test";
}

export function datasetCacheRoot(): string | undefined {
  if (datasetCacheDisabled()) {
    return undefined;
  }
  const override = readEnvOptional(DATASET_CACHE_DIR_ENV);
  if (override !== undefined) {
    return override;
  }
  return join(homedir(), ".cache", "openrouter-bench-harness");
}

export function encodeCacheKeySegment(segment: string): string {
  return encodeURIComponent(segment);
}

export interface ReadJsonCacheFileOptions {
  readonly maxAgeMs?: number;
  readonly now?: number;
}

export function readJsonCacheFile(
  path: string,
  opts?: ReadJsonCacheFileOptions
): unknown | undefined {
  try {
    if (opts?.maxAgeMs !== undefined) {
      const { mtimeMs } = statSync(path);
      const now = opts.now ?? Date.now();
      const ageMs = Math.max(0, now - mtimeMs);
      if (ageMs >= opts.maxAgeMs) {
        return undefined;
      }
    }
    const parsed: unknown = JSON.parse(readFileSync(path, "utf8"));
    return parsed;
  } catch {
    return undefined;
  }
}

export function mkdirOwnerOnly(path: string): void {
  const missing: string[] = [];
  let current = path;
  while (!existsSync(current)) {
    missing.push(current);
    const parent = dirname(current);
    if (parent === current) {
      break;
    }
    current = parent;
  }
  mkdirSync(path, { recursive: true });
  for (const dir of [path, ...missing]) {
    try {
      chmodSync(dir, 0o700);
    } catch (error) {
      wLog("cache dir permission restriction failed", {
        dir,
        error: String(error),
      });
    }
  }
}

export function sweepStaleStagingDirs(
  dir: string,
  maxAgeMs: number = STALE_STAGING_MAX_AGE_MS
): void {
  try {
    const now = Date.now();
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (!entry.name.includes(".staging-")) {
        continue;
      }
      const path = join(dir, entry.name);
      try {
        const { mtimeMs } = statSync(path);
        if (now - mtimeMs >= maxAgeMs) {
          removeDirRecursive(path);
        }
      } catch {
        wLog("stale staging dir sweep skipped an unreadable entry", {
          path,
        });
      }
    }
  } catch {
    wLog("stale staging dir sweep failed", { dir });
  }
}

export function writeJsonCacheFileAtomic(path: string, value: unknown): void {
  try {
    mkdirOwnerOnly(dirname(path));
    const tmp = `${path}.${process.pid}.${Math.random().toString(36).slice(2)}.tmp`;
    writeFileSync(tmp, JSON.stringify(value), { mode: 0o600 });
    renameSync(tmp, path);
  } catch (error) {
    wLog("dataset cache write failed", { path, error: String(error) });
  }
}

export function removeDirRecursive(path: string): void {
  rmSync(path, { recursive: true, force: true });
}

export function writeCheckoutCompleteMarker(root: string): void {
  try {
    writeFileSync(join(root, CHECKOUT_COMPLETE_MARKER), "", { mode: 0o600 });
  } catch (error) {
    wLog("checkout completion marker write failed", {
      root,
      error: String(error),
    });
  }
}

export function hasCheckoutCompleteMarker(root: string): boolean {
  try {
    return statSync(join(root, CHECKOUT_COMPLETE_MARKER)).isFile();
  } catch {
    return false;
  }
}

export function restrictPermissionsRecursive(root: string): void {
  try {
    restrictDirPermissions(root);
  } catch (error) {
    wLog("cache permission restriction failed", {
      root,
      error: String(error),
    });
  }
}

function restrictDirPermissions(dir: string): void {
  chmodSync(dir, 0o700);
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      restrictDirPermissions(path);
    } else if (entry.isFile()) {
      const ownerExec = statSync(path).mode & 0o100;
      chmodSync(path, ownerExec ? 0o700 : 0o600);
    }
  }
}

export function publishStagedCheckout(
  staging: string,
  shared: string,
  hasTasks: () => boolean
): string {
  const sharedIsComplete = () =>
    hasCheckoutCompleteMarker(shared) && hasTasks();
  try {
    renameSync(staging, shared);
    return shared;
  } catch (error) {
    if (sharedIsComplete()) {
      removeDirRecursive(staging);
      return shared;
    }
    if (hasCheckoutCompleteMarker(shared)) {
      removeDirRecursive(staging);
      throw new Error(
        `shared checkout exists but is incomplete; remove it manually: ${shared}`,
        { cause: error }
      );
    }
    try {
      removeDirRecursive(shared);
      renameSync(staging, shared);
      return shared;
    } catch {
      if (sharedIsComplete()) {
        removeDirRecursive(staging);
        return shared;
      }
      removeDirRecursive(staging);
      throw error;
    }
  }
}
