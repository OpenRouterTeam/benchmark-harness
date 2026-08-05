import { Either } from "../../internal/either";
import type {
  BenchmarkCliContext,
  BenchmarkCliPlugin,
  BenchmarkCliResolution,
} from "../types";
import type { DracoConfigOverride } from "./config-overlay";
import { persistRunConfig, resolveDracoRunConfig } from "./config-overlay";

function get(argv: readonly string[], flag: string): string | undefined {
  const idx = argv.indexOf(flag);
  return idx !== -1 ? argv[idx + 1] : undefined;
}

function num(argv: readonly string[], flag: string): number | undefined {
  const raw = get(argv, flag);
  return raw !== undefined ? Number(raw) : undefined;
}

function parseDracoOverride(argv: readonly string[]): DracoConfigOverride {
  const panelModels = get(argv, "--panel-models");
  const synthesisModel = get(argv, "--synthesis-model");
  const judgeModel = get(argv, "--judge-model");
  const judgeRuns = num(argv, "--judge-runs");
  const cacheNamespace = get(argv, "--cache-namespace");
  return {
    ...(panelModels !== undefined && {
      panelModels: panelModels
        .split(",")
        .map((s) => s.trim())
        .filter((s) => s.length > 0),
    }),
    ...(synthesisModel !== undefined && { synthesisModel }),
    ...(judgeModel !== undefined && { judgeModel }),
    ...(judgeRuns !== undefined && { judgeRuns }),
    ...(cacheNamespace !== undefined && { cacheNamespace }),
  };
}

export const DRACO_CLI_PLUGIN: BenchmarkCliPlugin = {
  resolve: async (
    ctx: BenchmarkCliContext
  ): Promise<BenchmarkCliResolution> => {
    const override = parseDracoOverride(ctx.argv);
    const resolved = await resolveDracoRunConfig({
      benchmarkConfig: ctx.benchmarkConfig,
      ...(ctx.resumeId !== undefined && { resumeDir: ctx.resumeId }),
      ...(Object.keys(override).length > 0 && { override }),
      ...(ctx.artifactDir !== undefined && { artifactDir: ctx.artifactDir }),
    });
    if (Either.isLeft(resolved)) {
      throw new Error(resolved.left);
    }
    const persisted = await persistRunConfig(
      resolved.right.artifactDir,
      resolved.right.config
    );
    if (Either.isLeft(persisted)) {
      process.stderr.write(`warning: ${persisted.left}\n`);
    }
    return {
      benchmarkConfig: resolved.right.config,
      artifactDir: resolved.right.artifactDir,
    };
  },
};
