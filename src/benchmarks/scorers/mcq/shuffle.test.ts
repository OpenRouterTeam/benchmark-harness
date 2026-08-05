import { describe, expect, it } from "bun:test";

import { seededPermutation } from "./shuffle";
describe("seededPermutation", () => {
  it("returns a valid permutation of [0, length)", () => {
    const perm = seededPermutation(4, 123);
    expect([...perm].sort()).toEqual([0, 1, 2, 3]);
  });
  it("is deterministic for the same seed and length", () => {
    expect(seededPermutation(4, 42)).toEqual(seededPermutation(4, 42));
  });
  it("produces different orderings for different seeds", () => {
    const orderings = new Set(
      Array.from({ length: 10 }, (_, seed) =>
        JSON.stringify(seededPermutation(4, seed))
      )
    );
    expect(orderings.size).toBeGreaterThan(1);
  });
  it("spreads the first element across positions for sequential seeds", () => {
    const positions = new Set(
      Array.from({ length: 20 }, (_, seed) =>
        seededPermutation(4, seed).indexOf(0)
      )
    );
    expect(positions.size).toBeGreaterThan(1);
  });
  it("handles length 1", () => {
    expect(seededPermutation(1, 5)).toEqual([0]);
  });
});
