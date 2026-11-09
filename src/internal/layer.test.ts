import { describe, expect, it } from "bun:test";

import { Service } from "effect/Context";
import { provide, result, runSync, succeed } from "effect/Effect";

import { layerFail } from "./layer";
import { assertFailure } from "./testing";

class Probe extends Service<Probe, { readonly value: number }>()(
  "@openrouter/bench-harness/internal/layer-test/Probe"
) {}

describe("layerFail", () => {
  it("fails the effect that depends on the layer with the given error", () => {
    const outcome = runSync(
      result(provide(succeed("unreachable"), layerFail<string, Probe>("nope")))
    );
    assertFailure(outcome);
    expect(outcome.failure).toBe("nope");
  });
});
