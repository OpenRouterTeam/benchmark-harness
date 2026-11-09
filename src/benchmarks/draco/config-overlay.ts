import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, join } from "node:path";

import * as Result from "effect/Result";

import { definedValues, isRecord } from "../../internal/guards";
import type { AsyncResult } from "../../internal/result";
import { tryPromiseResult } from "../../internal/result";
import { parseSchema } from "../../internal/zod";
import type { DracoPanelConfig } from "./schemas";
import { DracoPanelConfigSchema } from "./schemas";

export const CONFIG_FILENAME = "config.json";

export interface DracoConfigOverride {
  readonly panelModels?: readonly string[];
  readonly synthesisModel?: string;
  readonly model?: string;
  readonly judgeModel?: string;
  readonly judgeRuns?: number;
  readonly versionOverride?: string;
  readonly cacheNamespace?: string;
}

export function applyConfigOverlay(
  base: DracoPanelConfig,
  override: DracoConfigOverride
): Result.Result<DracoPanelConfig, string> {
  const merged: DracoPanelConfig = {
    ...base,
    ...definedValues({
      analysisModels:
        override.panelModels !== undefined
          ? [...override.panelModels]
          : undefined,
      synthesisModel: override.synthesisModel,
      model: override.model,
      judgeModel: override.judgeModel,
      judgeRuns: override.judgeRuns,
      versionOverride: override.versionOverride,
      cacheNamespace: override.cacheNamespace,
    }),
  };
  const parsed = parseSchema(DracoPanelConfigSchema, merged);
  if (Result.isFailure(parsed)) {
    return Result.fail(
      `Invalid DRACO config overlay: ${parsed.failure.message}`
    );
  }
  return Result.succeed(parsed.success);
}

export async function loadResumeConfig(
  artifactDir: string
): AsyncResult<DracoPanelConfig, string> {
  const path = join(artifactDir, CONFIG_FILENAME);
  const read = await tryPromiseResult(() => readFile(path, "utf8"));
  if (Result.isFailure(read)) {
    return Result.fail(
      `Could not read resume config at ${path}: ${String(read.failure)}`
    );
  }
  const json = Result.try(() => JSON.parse(read.success));
  if (Result.isFailure(json)) {
    return Result.fail(
      `Resume config at ${path} is not valid JSON: ${String(json.failure)}`
    );
  }
  if (isRecord(json.success) && json.success["fusionMode"] === "fusion-lib") {
    return Result.fail(
      `Resume config at ${path} uses removed DRACO fusion mode "fusion-lib"; rerun with a production config`
    );
  }
  const parsed = parseSchema(DracoPanelConfigSchema, json.success);
  if (Result.isFailure(parsed)) {
    return Result.fail(
      `Resume config at ${path} failed validation: ${parsed.failure.message}`
    );
  }
  return Result.succeed(parsed.success);
}

export async function resolveDracoRunConfig(input: {
  readonly benchmarkConfig?: unknown;
  readonly resumeDir?: string;
  readonly override?: DracoConfigOverride;
  readonly artifactDir?: string;
}): AsyncResult<
  {
    readonly config: DracoPanelConfig;
    readonly artifactDir?: string;
  },
  string
> {
  if (input.resumeDir !== undefined) {
    const base = await loadResumeConfig(input.resumeDir);
    if (Result.isFailure(base)) {
      return Result.fail(base.failure);
    }
    const override = input.override ?? {};
    const hasOverride = Object.keys(override).length > 0;
    if (!hasOverride) {
      return Result.succeed({
        config: base.success,
        artifactDir: input.resumeDir,
      });
    }
    const merged = applyConfigOverlay(base.success, override);
    if (Result.isFailure(merged)) {
      return Result.fail(merged.failure);
    }
    return Result.succeed({
      config: merged.success,
      artifactDir: input.resumeDir,
    });
  }
  const resolved = parseSchema(DracoPanelConfigSchema, input.benchmarkConfig);
  if (Result.isFailure(resolved)) {
    return Result.fail(
      `Invalid DRACO panel config: ${resolved.failure.message}`
    );
  }
  const override = input.override ?? {};
  const hasOverride = Object.keys(override).length > 0;
  if (!hasOverride) {
    return Result.succeed({
      config: resolved.success,
      artifactDir: input.artifactDir,
    });
  }
  const merged = applyConfigOverlay(resolved.success, override);
  if (Result.isFailure(merged)) {
    return Result.fail(merged.failure);
  }
  return Result.succeed({
    config: merged.success,
    artifactDir: input.artifactDir,
  });
}

export async function persistRunConfig(
  artifactDir: string | undefined,
  config: DracoPanelConfig
): AsyncResult<void, string> {
  if (artifactDir === undefined) {
    return Result.succeed(undefined);
  }
  const path = join(artifactDir, CONFIG_FILENAME);
  const r = await tryPromiseResult(async () => {
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, JSON.stringify(config, null, 2), "utf8");
  });
  return Result.isFailure(r)
    ? Result.fail(`Failed to write run config to ${path}: ${String(r.failure)}`)
    : Result.succeed(undefined);
}
