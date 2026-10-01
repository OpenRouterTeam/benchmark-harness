import { describe, expect, it } from "bun:test";

import { stripRoutingPrefix } from "./model-slug";

describe("stripRoutingPrefix", () => {
  it("removes the openrouter/ routing prefix from a full slug", () => {
    expect(stripRoutingPrefix("openrouter/sakana/fugu-ultra")).toBe(
      "sakana/fugu-ultra"
    );
  });

  it("keeps openrouter-authored slugs and unprefixed slugs unchanged", () => {
    expect(stripRoutingPrefix("openrouter/auto")).toBe("openrouter/auto");
    expect(stripRoutingPrefix("sakana/fugu-ultra")).toBe("sakana/fugu-ultra");
  });
});
