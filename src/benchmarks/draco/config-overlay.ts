import { readFile, writeFile, mkdir } from "node:fs/promises";
import { dirname, join } from "node:path";

import type { AsyncEither } from "../../internal/either";
import { Either, tryPromiseEither } from "../../internal/either";
import { definedValues, isRecord } from "../../internal/guards";
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
): Either.Either<DracoPanelConfig, string> {
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
  if (Either.isLeft(parsed)) {
    return Either.left(`Invalid DRACO config overlay: ${parsed.left.message}`);
  }
  return Either.right(parsed.right);
}

export async function loadResumeConfig(
  artifactDir: string
): AsyncEither<DracoPanelConfig, string> {
  const path = join(artifactDir, CONFIG_FILENAME);
  const read = await tryPromiseEither(() => readFile(path, "utf8"));
  if (Either.isLeft(read)) {
    return Either.left(
      `Could not read resume config at ${path}: ${String(read.left)}`
    );
  }
  const json = Either.try(() => JSON.parse(read.right));
  if (Either.isLeft(json)) {
    return Either.left(
      `Resume config at ${path} is not valid JSON: ${String(json.left)}`
    );
  }
  if (isRecord(json.right) && json.right["fusionMode"] === "fusion-lib") {
    return Either.left(
      `Resume config at ${path} uses removed DRACO fusion mode "fusion-lib"; rerun with a production config`
    );
  }
  const parsed = parseSchema(DracoPanelConfigSchema, json.right);
  if (Either.isLeft(parsed)) {
    return Either.left(
      `Resume config at ${path} failed validation: ${parsed.left.message}`
    );
  }
  return Either.right(parsed.right);
}

export async function resolveDracoRunConfig(input: {
  readonly benchmarkConfig?: unknown;
  readonly resumeDir?: string;
  readonly override?: DracoConfigOverride;
  readonly artifactDir?: string;
}): AsyncEither<
  {
    readonly config: DracoPanelConfig;
    readonly artifactDir?: string;
  },
  string
> {
  if (input.resumeDir !== undefined) {
    const base = await loadResumeConfig(input.resumeDir);
    if (Either.isLeft(base)) {
      return Either.left(base.left);
    }
    const override = input.override ?? {};
    const hasOverride = Object.keys(override).length > 0;
    if (!hasOverride) {
      return Either.right({ config: base.right, artifactDir: input.resumeDir });
    }
    const merged = applyConfigOverlay(base.right, override);
    if (Either.isLeft(merged)) {
      return Either.left(merged.left);
    }
    return Either.right({ config: merged.right, artifactDir: input.resumeDir });
  }
  const resolved = parseSchema(DracoPanelConfigSchema, input.benchmarkConfig);
  if (Either.isLeft(resolved)) {
    return Either.left(`Invalid DRACO panel config: ${resolved.left.message}`);
  }
  const override = input.override ?? {};
  const hasOverride = Object.keys(override).length > 0;
  if (!hasOverride) {
    return Either.right({
      config: resolved.right,
      artifactDir: input.artifactDir,
    });
  }
  const merged = applyConfigOverlay(resolved.right, override);
  if (Either.isLeft(merged)) {
    return Either.left(merged.left);
  }
  return Either.right({ config: merged.right, artifactDir: input.artifactDir });
}

export async function persistRunConfig(
  artifactDir: string | undefined,
  config: DracoPanelConfig
): AsyncEither<void, string> {
  if (artifactDir === undefined) {
    return Either.right(undefined);
  }
  const path = join(artifactDir, CONFIG_FILENAME);
  const r = await tryPromiseEither(async () => {
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, JSON.stringify(config, null, 2), "utf8");
  });
  return Either.isLeft(r)
    ? Either.left(`Failed to write run config to ${path}: ${String(r.left)}`)
    : Either.right(undefined);
}
