import { describe, expect, it } from "bun:test";

import { assertLeft, assertRight } from "../../../internal/testing";
import { alignmentJudgeSpec, cellJudgeSpec } from "./judges";
describe("WideSearch judge specs", () => {
  it("accepts partial, duplicate, and out-of-vocabulary alignments", () => {
    const spec = alignmentJudgeSpec(["Alpha", "Beta"], ["A", "B"]);
    const valid = spec.parseVerdict(
      '{"alignments":[{"origin":"Alpha","transform":"A"}]}'
    );
    const unexpected = spec.parseVerdict(
      '{"alignments":[{"origin":"Other","transform":"A"}]}'
    );
    const duplicate = spec.parseVerdict(
      '{"alignments":[{"origin":"Alpha","transform":"A"},{"origin":"Alpha","transform":"B"}]}'
    );
    const unknownTransform = spec.parseVerdict(
      '{"alignments":[{"origin":"Alpha","transform":"Other"}]}'
    );
    const malformed = spec.parseVerdict(
      '{"alignments":[{"origin":"Alpha","transform":1}]}'
    );
    assertRight(valid);
    assertRight(unexpected);
    assertRight(duplicate);
    assertRight(unknownTransform);
    assertLeft(malformed);
    expect(valid.right).toEqual([{ origin: "Alpha", transform: "A" }]);
    expect(duplicate.right).toEqual([
      { origin: "Alpha", transform: "A" },
      { origin: "Alpha", transform: "B" },
    ]);
  });
  it("accepts missing, out-of-range, and duplicate cell indices", () => {
    const spec = cellJudgeSpec(["a", "b"], ["x", "y"], "criterion");
    const valid = spec.parseVerdict('{"scores":[{"index":0,"score":1}]}');
    const outOfRange = spec.parseVerdict('{"scores":[{"index":2,"score":1}]}');
    const duplicate = spec.parseVerdict(
      '{"scores":[{"index":0,"score":1},{"index":0,"score":0}]}'
    );
    const malformed = spec.parseVerdict(
      '{"scores":[{"index":0,"score":2}]}'
    );
    assertRight(valid);
    assertRight(outOfRange);
    assertRight(duplicate);
    assertLeft(malformed);
    expect(valid.right).toEqual([{ index: 0, score: 1 }]);
    expect(duplicate.right).toEqual([
      { index: 0, score: 1 },
      { index: 0, score: 0 },
    ]);
  });
  it("uses fixed schemas for large verdict sets", () => {
    const observed = Array.from(
      { length: 1000 },
      (_, index) => `value-${index}`
    );
    const alignment = alignmentJudgeSpec(observed, observed);
    const cells = cellJudgeSpec(observed, observed, "criterion");
    const alignmentVerdict = alignment.parseVerdict('{"alignments":[]}');
    const cellVerdict = cells.parseVerdict('{"scores":[]}');
    assertRight(alignmentVerdict);
    assertRight(cellVerdict);
    expect(JSON.stringify(alignment.jsonSchema).length).toBeLessThan(500);
    expect(JSON.stringify(cells.jsonSchema).length).toBeLessThan(500);
    expect(alignment.jsonSchema).toMatchObject({
      type: "object",
      additionalProperties: false,
      required: ["alignments"],
      properties: {
        alignments: {
          items: {
            type: "object",
            additionalProperties: false,
            required: ["origin", "transform"],
          },
        },
      },
    });
    expect(cells.jsonSchema).toMatchObject({
      type: "object",
      additionalProperties: false,
      required: ["scores"],
      properties: {
        scores: {
          items: {
            type: "object",
            additionalProperties: false,
            required: ["index", "score"],
          },
        },
      },
    });
    expect(alignmentVerdict.right).toEqual([]);
    expect(cellVerdict.right).toEqual([]);
  });
  it("renders alignment values literally without replacement-string expansion", () => {
    const spec = alignmentJudgeSpec(
      ["$&", "{reference}"],
      ["$$", "{response}"]
    );
    expect(spec.userInput).toContain('["$&","{reference}"]');
    expect(spec.userInput).toContain('["$$","{response}"]');
  });
  it("renders cell values and criteria literally without replacement-string expansion", () => {
    const spec = cellJudgeSpec(
      ["$&", "$$"],
      ["{criterion}", "{response}"],
      "$` and $'"
    );
    expect(spec.userInput).toContain("$` and $'");
    expect(spec.userInput).toContain(
      '{"idx_0":{"response":"$&","target":"{criterion}"},"idx_1":{"response":"$$","target":"{response}"}}'
    );
  });
});
