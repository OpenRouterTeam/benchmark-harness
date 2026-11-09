import { describe, expect, it } from "bun:test";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { assertSuccess, assertFailure } from "../../internal/testing";
import {
  applyConfigOverlay,
  loadResumeConfig,
  persistRunConfig,
  resolveDracoRunConfig,
} from "./config-overlay";
import { experimentTools } from "./request-body";
import type { DracoPanelConfig } from "./schemas";

function baseConfig(
  overrides: Partial<DracoPanelConfig> = {}
): DracoPanelConfig {
  return {
    name: "fusion-2x",
    description: "",
    type: "fusion",
    synthesisModel: "openai/gpt-4o-mini",
    analysisModels: ["openai/gpt-4o-mini", "google/gemini-2.5-flash"],
    searchEngine: "exa",
    blockedDomains: [],
    judgeModel: "google/gemini-3.1-pro-preview",
    judgeRuns: 3,
    judgeReasoningEffort: "low",
    criterionConcurrency: 10,
    timeout: 1800,
    concurrency: 2,
    ...overrides,
  };
}
describe("applyConfigOverlay", () => {
  it("replaces the panel wholesale (Q replaces Z, X and Y kept by the user listing them)", () => {
    const merged = applyConfigOverlay(baseConfig(), {
      panelModels: [
        "openai/gpt-4o-mini",
        "google/gemini-2.5-flash",
        "anthropic/claude-opus-5",
      ],
    });
    assertSuccess(merged);
    expect(merged.success.analysisModels).toEqual([
      "openai/gpt-4o-mini",
      "google/gemini-2.5-flash",
      "anthropic/claude-opus-5",
    ]);
    expect(merged.success.synthesisModel).toBe("openai/gpt-4o-mini");
    expect(merged.success.judgeModel).toBe("google/gemini-3.1-pro-preview");
  });
  it("swaps the fusion analysis/judge model parameter only", () => {
    const merged = applyConfigOverlay(baseConfig(), {
      synthesisModel: "anthropic/claude-opus-5",
    });
    assertSuccess(merged);
    expect(merged.success.synthesisModel).toBe("anthropic/claude-opus-5");
    expect(merged.success.analysisModels).toEqual([
      "openai/gpt-4o-mini",
      "google/gemini-2.5-flash",
    ]);
  });
  it("swaps the judge model only (the generation-reuse case)", () => {
    const merged = applyConfigOverlay(baseConfig(), {
      judgeModel: "openai/gpt-5.5",
    });
    assertSuccess(merged);
    expect(merged.success.judgeModel).toBe("openai/gpt-5.5");
    expect(merged.success.synthesisModel).toBe("openai/gpt-4o-mini");
  });
  it("re-validates and rejects an overlay exceeding the panel size cap", () => {
    const merged = applyConfigOverlay(baseConfig(), {
      panelModels: ["a", "b", "c", "d", "e"],
    });
    assertFailure(merged);
  });
  it("sets the cache namespace (opt into stage reuse)", () => {
    const merged = applyConfigOverlay(baseConfig(), {
      cacheNamespace: "sweep-1",
    });
    assertSuccess(merged);
    expect(merged.success.cacheNamespace).toBe("sweep-1");
  });
});
describe("resolveDracoRunConfig", () => {
  it("errors when benchmarkConfig is not supplied", async () => {
    const resolved = await resolveDracoRunConfig({});
    assertFailure(resolved);
    expect(resolved.failure).toContain("Invalid DRACO panel config");
  });
  it("applies an override on a fresh benchmarkConfig object", async () => {
    const resolved = await resolveDracoRunConfig({
      benchmarkConfig: baseConfig(),
      override: { judgeModel: "openai/gpt-5.5" },
    });
    assertSuccess(resolved);
    expect(resolved.success.config.type).toBe("fusion");
    expect(resolved.success.config.judgeModel).toBe("openai/gpt-5.5");
  });
  it("loads a resume config and applies the override on top", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-resume-"));
    try {
      const base = baseConfig();
      assertSuccess(await persistRunConfig(dir, base));
      const resolved = await resolveDracoRunConfig({
        resumeDir: dir,
        override: {
          panelModels: [
            "openai/gpt-4o-mini",
            "google/gemini-2.5-flash",
            "x-ai/grok-4",
          ],
        },
      });
      assertSuccess(resolved);
      expect(resolved.success.artifactDir).toBe(dir);
      expect(resolved.success.config.analysisModels).toEqual([
        "openai/gpt-4o-mini",
        "google/gemini-2.5-flash",
        "x-ai/grok-4",
      ]);
      expect(resolved.success.config.judgeModel).toBe(base.judgeModel);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("returns the resume config unchanged when no override is given", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-resume-"));
    try {
      const base = baseConfig({ judgeModel: "openai/gpt-5.5" });
      assertSuccess(await persistRunConfig(dir, base));
      const resolved = await resolveDracoRunConfig({ resumeDir: dir });
      assertSuccess(resolved);
      expect(resolved.success.config.judgeModel).toBe("openai/gpt-5.5");
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("loads an existing production resume config with the removed mode field", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-resume-"));
    try {
      await writeFile(
        join(dir, "config.json"),
        JSON.stringify({ ...baseConfig(), fusionMode: "production" })
      );
      const resolved = await resolveDracoRunConfig({ resumeDir: dir });
      assertSuccess(resolved);
      expect(resolved.success.config).toEqual(baseConfig());
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("loads a legacy shell config without forwarding its removed idle timeout", async () => {
    const directory = await mkdtemp(join(tmpdir(), "draco-resume-"));
    const parameters = {
      engine: "openrouter",
      environment: { type: "container_reference", containerId: "sess_test" },
    } as const;
    const tools = [{ type: "openrouter:shell", parameters }];
    try {
      await writeFile(
        join(directory, "config.json"),
        JSON.stringify({
          ...baseConfig(),
          tools: [
            {
              type: "openrouter:shell",
              parameters: { ...parameters, sleepAfterSeconds: 3600 },
            },
          ],
        })
      );

      const resolved = await resolveDracoRunConfig({ resumeDir: directory });

      assertSuccess(resolved);
      expect(resolved.success.config.tools).toEqual(tools);
      expect(experimentTools(resolved.success.config)).toEqual(tools);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  });
  it("errors on a missing resume dir", async () => {
    const resolved = await resolveDracoRunConfig({
      resumeDir: "/does/not/exist-draco",
    });
    assertFailure(resolved);
  });
  it("rejects resume configs that use the removed fusion-lib mode", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-resume-"));
    try {
      await writeFile(
        join(dir, "config.json"),
        JSON.stringify({ ...baseConfig(), fusionMode: "fusion-lib" })
      );
      const resolved = await resolveDracoRunConfig({ resumeDir: dir });
      assertFailure(resolved);
      expect(resolved.failure).toContain(
        'uses removed DRACO fusion mode "fusion-lib"'
      );
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
});
describe("persistRunConfig + loadResumeConfig round-trip", () => {
  it("writes then reads back a config unchanged", async () => {
    const d = await mkdtemp(join(tmpdir(), "draco-rt-"));
    try {
      const base = baseConfig();
      assertSuccess(await persistRunConfig(d, base));
      const loaded = await loadResumeConfig(d);
      assertSuccess(loaded);
      expect(loaded.success).toEqual(base);
    } finally {
      await rm(d, { recursive: true, force: true });
    }
  });
  it("persistRunConfig is a no-op (ok) when artifactDir is undefined", async () => {
    const r = await persistRunConfig(undefined, baseConfig());
    assertSuccess(r);
  });
});
