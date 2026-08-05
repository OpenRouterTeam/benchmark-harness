import { describe, expect, it } from "bun:test";

import { assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { IFSTRUCT_BENCHMARK, ifStructRecordToSample } from "./benchmark";
import type { IfStructRequirements } from "./schema";
import { IfStructRequirementsSchema } from "./schema";

const BASE_RECORD = {
  doc_id: 42,
  entity_type: "recipe",
  prompt: "List two ingredients as JSON.",
  output_format: "json",
  top_level_count: "2",
  top_level_key: "ingredients",
  require_wrapper_key: true,
  require_code_block: true,
  require_no_commentary: true,
  json_schema:
    '{"type": "array", "items": {"type": "object", "properties": {"id": {"type": "string"}}}}',
} as const;

function requirementsOf(
  metadata: Readonly<Record<string, unknown>> | undefined
): IfStructRequirements {
  const parsed = parseSchema(IfStructRequirementsSchema, metadata);
  assertRight(parsed);
  return parsed.right;
}
describe("ifStructRecordToSample", () => {
  it("derives a stable id and maps the prompt/target", () => {
    const sample = ifStructRecordToSample(BASE_RECORD);
    expect(sample.id).toBe("ifstruct-42");
    expect(sample.input).toBe("List two ingredients as JSON.");
    expect(sample.target).toEqual({ text: "json" });
  });
  it("parses json_schema and top_level_count strings into structured requirements", () => {
    const requirements = requirementsOf(
      ifStructRecordToSample(BASE_RECORD).metadata
    );
    expect(requirements.topLevelCount).toBe(2);
    expect(requirements.topLevelKey).toBe("ingredients");
    expect(requirements.requireWrapperKey).toBe(true);
    expect(requirements.jsonSchema.type).toBe("array");
  });
  it('parses a "[min, max]" range for top_level_count', () => {
    const requirements = requirementsOf(
      ifStructRecordToSample({ ...BASE_RECORD, top_level_count: "[3, 5]" })
        .metadata
    );
    expect(requirements.topLevelCount).toEqual([3, 5]);
  });
  it("treats an empty top_level_key as null", () => {
    const requirements = requirementsOf(
      ifStructRecordToSample({ ...BASE_RECORD, top_level_key: "" }).metadata
    );
    expect(requirements.topLevelKey).toBeNull();
  });
  it("treats an empty top_level_count as null (no constraint)", () => {
    const requirements = requirementsOf(
      ifStructRecordToSample({ ...BASE_RECORD, top_level_count: "" }).metadata
    );
    expect(requirements.topLevelCount).toBeNull();
  });
  it("throws when a required field is missing", () => {
    const { prompt: _prompt, ...withoutPrompt } = BASE_RECORD;
    expect(() => ifStructRecordToSample(withoutPrompt)).toThrow(
      /failed validation/
    );
  });
  it("throws when json_schema is not valid JSON", () => {
    expect(() =>
      ifStructRecordToSample({ ...BASE_RECORD, json_schema: "not json" })
    ).toThrow(/json_schema is not valid JSON/);
  });
});
describe("IFSTRUCT_BENCHMARK", () => {
  it("exposes a temperature-0, single-epoch definition", () => {
    expect(IFSTRUCT_BENCHMARK.id).toBe("ifstruct");
    expect(IFSTRUCT_BENCHMARK.temperature).toBe(0);
    expect(IFSTRUCT_BENCHMARK.defaultEpochs).toBe(1);
    expect(typeof IFSTRUCT_BENCHMARK.makeLayer).toBe("function");
    expect(typeof IFSTRUCT_BENCHMARK.makeDatasetLayer).toBe("function");
  });
});
