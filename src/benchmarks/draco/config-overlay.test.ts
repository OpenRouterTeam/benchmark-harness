import { describe, expect, it } from "bun:test";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { assertRight, assertLeft } from "../../internal/testing";
import {
  applyConfigOverlay,
  loadResumeConfig,
  persistRunConfig,
  resolveDracoRunConfig,
} from "./config-overlay";
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
    assertRight(merged);
    expect(merged.right.analysisModels).toEqual([
      "openai/gpt-4o-mini",
      "google/gemini-2.5-flash",
      "anthropic/claude-opus-5",
    ]);
    expect(merged.right.synthesisModel).toBe("openai/gpt-4o-mini");
    expect(merged.right.judgeModel).toBe("google/gemini-3.1-pro-preview");
  });
  it("swaps the fusion analysis/judge model parameter only", () => {
    const merged = applyConfigOverlay(baseConfig(), {
      synthesisModel: "anthropic/claude-opus-5",
    });
    assertRight(merged);
    expect(merged.right.synthesisModel).toBe("anthropic/claude-opus-5");
    expect(merged.right.analysisModels).toEqual([
      "openai/gpt-4o-mini",
      "google/gemini-2.5-flash",
    ]);
  });
  it("swaps the judge model only (the generation-reuse case)", () => {
    const merged = applyConfigOverlay(baseConfig(), {
      judgeModel: "openai/gpt-5.5",
    });
    assertRight(merged);
    expect(merged.right.judgeModel).toBe("openai/gpt-5.5");
    expect(merged.right.synthesisModel).toBe("openai/gpt-4o-mini");
  });
  it("re-validates and rejects an overlay exceeding the panel size cap", () => {
    const merged = applyConfigOverlay(baseConfig(), {
      panelModels: ["a", "b", "c", "d", "e"],
    });
    assertLeft(merged);
  });
  it("sets the cache namespace (opt into stage reuse)", () => {
    const merged = applyConfigOverlay(baseConfig(), {
      cacheNamespace: "sweep-1",
    });
    assertRight(merged);
    expect(merged.right.cacheNamespace).toBe("sweep-1");
  });
});
describe("resolveDracoRunConfig", () => {
  it("errors when benchmarkConfig is not supplied", async () => {
    const resolved = await resolveDracoRunConfig({});
    assertLeft(resolved);
    expect(resolved.left).toContain("Invalid DRACO panel config");
  });
  it("applies an override on a fresh benchmarkConfig object", async () => {
    const resolved = await resolveDracoRunConfig({
      benchmarkConfig: baseConfig(),
      override: { judgeModel: "openai/gpt-5.5" },
    });
    assertRight(resolved);
    expect(resolved.right.config.type).toBe("fusion");
    expect(resolved.right.config.judgeModel).toBe("openai/gpt-5.5");
  });
  it("loads a resume config and applies the override on top", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-resume-"));
    try {
      const base = baseConfig();
      assertRight(await persistRunConfig(dir, base));
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
      assertRight(resolved);
      expect(resolved.right.artifactDir).toBe(dir);
      expect(resolved.right.config.analysisModels).toEqual([
        "openai/gpt-4o-mini",
        "google/gemini-2.5-flash",
        "x-ai/grok-4",
      ]);
      expect(resolved.right.config.judgeModel).toBe(base.judgeModel);
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("returns the resume config unchanged when no override is given", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-resume-"));
    try {
      const base = baseConfig({ judgeModel: "openai/gpt-5.5" });
      assertRight(await persistRunConfig(dir, base));
      const resolved = await resolveDracoRunConfig({ resumeDir: dir });
      assertRight(resolved);
      expect(resolved.right.config.judgeModel).toBe("openai/gpt-5.5");
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
      assertRight(resolved);
      expect(resolved.right.config).toEqual(baseConfig());
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  });
  it("errors on a missing resume dir", async () => {
    const resolved = await resolveDracoRunConfig({
      resumeDir: "/does/not/exist-draco",
    });
    assertLeft(resolved);
  });
  it("rejects resume configs that use the removed fusion-lib mode", async () => {
    const dir = await mkdtemp(join(tmpdir(), "draco-resume-"));
    try {
      await writeFile(
        join(dir, "config.json"),
        JSON.stringify({ ...baseConfig(), fusionMode: "fusion-lib" })
      );
      const resolved = await resolveDracoRunConfig({ resumeDir: dir });
      assertLeft(resolved);
      expect(resolved.left).toContain(
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
      assertRight(await persistRunConfig(d, base));
      const loaded = await loadResumeConfig(d);
      assertRight(loaded);
      expect(loaded.right).toEqual(base);
    } finally {
      await rm(d, { recursive: true, force: true });
    }
  });
  it("persistRunConfig is a no-op (ok) when artifactDir is undefined", async () => {
    const r = await persistRunConfig(undefined, baseConfig());
    assertRight(r);
  });
});
