import { describe, expect, it } from "bun:test";

import { stripVariantSuffix } from "./model-slug";

describe("stripVariantSuffix", () => {
  it.each([
    ["nvidia/switchyard:online", "nvidia/switchyard"],
    ["openai/gpt-5:free:online", "openai/gpt-5"],
    ["openai/gpt-5", "openai/gpt-5"],
    [":free", ":free"],
  ] as const)("strips the variant from %s", (model, expected) => {
    expect(stripVariantSuffix(model)).toBe(expected);
  });

  it("keeps effect/Context out of the benchmark config module graph", () => {
    const script = [
      `await import(${JSON.stringify(`${import.meta.dir}/../benchmarks/benchmark-config.ts`)});`,
      "const loaded = Object.keys(require.cache).filter((k) => k.endsWith('/effect/dist/esm/Context.js'));",
      "console.log(loaded.length);",
    ].join("\n");

    const result = Bun.spawnSync({ cmd: ["bun", "-e", script] });

    expect(result.stderr.toString()).toBe("");
    expect(result.stdout.toString().trim()).toBe("0");
  });
});
