import { describe, expect, it } from "bun:test";

import { MODAL_REGION_PINS, resolveModalRegions } from "./modal-regions";

describe("resolveModalRegions", () => {
  it("pins Fugu models to the US by default", () => {
    expect(resolveModalRegions("sakana/fugu-ultra", undefined)).toEqual(["us"]);
    expect(resolveModalRegions("sakana/fugu-ultra-v2", undefined)).toEqual([
      "us",
    ]);
    expect(resolveModalRegions("sakana/fugu-max", undefined)).toEqual(["us"]);
  });

  it("leaves unpinned models to Modal's default placement", () => {
    expect(
      resolveModalRegions("anthropic/claude-opus-5", undefined)
    ).toBeUndefined();
  });

  it("lets explicit config override the pin table, including opting out", () => {
    expect(resolveModalRegions("sakana/fugu-ultra", ["us-east"])).toEqual([
      "us-east",
    ]);
    expect(resolveModalRegions("sakana/fugu-ultra", [])).toEqual([]);
    expect(resolveModalRegions("openai/gpt-5.5", ["eu"])).toEqual(["eu"]);
  });

  it("returns the first matching pin from a custom table", () => {
    const pins = [
      { modelPrefix: "acme/", regions: ["eu"] },
      { modelPrefix: "acme/model", regions: ["us"] },
    ];
    expect(resolveModalRegions("acme/model", undefined, pins)).toEqual(["eu"]);
  });

  it("keeps the shipped pin table limited to Fugu", () => {
    expect(MODAL_REGION_PINS.map((pin) => pin.modelPrefix)).toEqual([
      "sakana/fugu-",
    ]);
  });
});
