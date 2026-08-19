import {
  mkdirSync,
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

export function writeJsonCacheFileAtomic(path: string, value: unknown): void {
  try {
    mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
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

export function publishStagedCheckout(
  staging: string,
  shared: string,
  sharedIsValid: () => boolean
): string {
  try {
    renameSync(staging, shared);
    return shared;
  } catch (error) {
    if (!sharedIsValid()) {
      removeDirRecursive(shared);
    }
    try {
      renameSync(staging, shared);
      return shared;
    } catch {
      if (sharedIsValid()) {
        removeDirRecursive(staging);
        return shared;
      }
      removeDirRecursive(staging);
      throw error;
    }
  }
}
