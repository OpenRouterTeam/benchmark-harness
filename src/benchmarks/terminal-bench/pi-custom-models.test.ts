import { describe, expect, it } from "bun:test";

import {
  buildPiModelsJson,
  findPiRouterModelDef,
  PI_UNKNOWN_ROUTER_MODELS,
} from "./pi-custom-models";
describe("findPiRouterModelDef", () => {
  it("finds phaser for the openrouter provider", () => {
    const def = findPiRouterModelDef("openrouter", "openrouter/phaser");
    expect(def?.id).toBe("openrouter/phaser");
  });
  it("returns undefined for concrete openrouter models pi already knows", () => {
    expect(
      findPiRouterModelDef("openrouter", "anthropic/claude-sonnet-4")
    ).toBeUndefined();
  });
  it("returns undefined for non-openrouter providers, even with a matching id", () => {
    expect(
      findPiRouterModelDef("anthropic", "openrouter/phaser")
    ).toBeUndefined();
  });
});
describe("buildPiModelsJson", () => {
  it("returns undefined for non-openrouter providers", () => {
    expect(buildPiModelsJson("openai", "gpt-4o")).toBeUndefined();
    expect(buildPiModelsJson("anthropic", "openrouter/phaser")).toBeUndefined();
  });
  it("generates a provider-level anthropic cache compat for concrete openrouter models, without model entries", () => {
    const json = buildPiModelsJson("openrouter", "z-ai/glm-5.2");
    if (json === undefined) {
      throw new Error("expected models.json for openrouter/z-ai/glm-5.2");
    }
    const parsed: unknown = JSON.parse(json);
    expect(parsed).toEqual({
      providers: {
        openrouter: {
          compat: {
            thinkingFormat: "openrouter",
            cacheControlFormat: "anthropic",
          },
        },
      },
    });
  });
  it("adds a provider-level session header for concrete openrouter models", () => {
    const json = buildPiModelsJson(
      "openrouter",
      "z-ai/glm-5.2",
      "workflow-123"
    );
    if (json === undefined) {
      throw new Error("expected models.json for openrouter/z-ai/glm-5.2");
    }
    const parsed: unknown = JSON.parse(json);
    expect(parsed).toEqual({
      providers: {
        openrouter: {
          headers: { "x-session-id": "workflow-123" },
          compat: {
            thinkingFormat: "openrouter",
            cacheControlFormat: "anthropic",
          },
        },
      },
    });
  });
  it("generates a valid models.json entry for openrouter/phaser", () => {
    const json = buildPiModelsJson(
      "openrouter",
      "openrouter/phaser",
      "workflow-123"
    );
    if (json === undefined) {
      throw new Error("expected models.json for openrouter/phaser");
    }
    const parsed: unknown = JSON.parse(json);
    expect(parsed).toEqual({
      providers: {
        openrouter: {
          headers: { "x-session-id": "workflow-123" },
          compat: {
            thinkingFormat: "openrouter",
            cacheControlFormat: "anthropic",
          },
          models: [
            {
              id: "openrouter/phaser",
              name: "OpenRouter: Phaser",
              reasoning: true,
              input: ["text"],
              cost: { input: 2, output: 10, cacheRead: 0.2, cacheWrite: 2.5 },
              contextWindow: 1000000,
              maxTokens: 128000,
              compat: {
                thinkingFormat: "openrouter",
                cacheControlFormat: "anthropic",
              },
            },
          ],
        },
      },
    });
  });
  it("mirrors the compat of pi built-in openrouter anthropic entries for every router model", () => {
    for (const def of PI_UNKNOWN_ROUTER_MODELS) {
      const json = buildPiModelsJson("openrouter", def.id);
      if (json === undefined) {
        throw new Error(`expected models.json for openrouter/${def.id}`);
      }
      expect(json).toContain('"cacheControlFormat": "anthropic"');
      expect(json).toContain('"thinkingFormat": "openrouter"');
    }
  });
  it("adds a session header for an unknown preset without adding a model entry", () => {
    const json = buildPiModelsJson(
      "openrouter",
      "@preset/advisor-terra-sol",
      "workflow-123"
    );
    if (json === undefined) {
      throw new Error("expected models.json for the openrouter preset");
    }
    const parsed: unknown = JSON.parse(json);
    expect(parsed).toEqual({
      providers: {
        openrouter: {
          headers: { "x-session-id": "workflow-123" },
          compat: {
            thinkingFormat: "openrouter",
            cacheControlFormat: "anthropic",
          },
        },
      },
    });
  });
});
