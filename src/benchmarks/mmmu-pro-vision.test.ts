import { afterEach, describe, expect, it } from "bun:test";

import { blockNetwork } from "../../test/helpers/block-network";
import { NOOP_PROGRESS_REPORTER } from "../harness/progress";
import { assertFailure, assertSuccess } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import { runBenchmarkById } from "../runner/run-by-id";
import { MmmuProVisionBenchmarkConfigSchema } from "./benchmark-config";
import {
  mmmuExtraBody,
  mmmuProVisionRecordToSample,
  mmmuProVisionSolverOptions,
} from "./mmmu-pro-vision";
import { getBenchmark } from "./registry";

const VISION_RECORD: Readonly<Record<string, unknown>> = {
  id: "test_History_1",
  image: { src: "https://example.com/history.png", height: 1182, width: 1014 },
  options:
    "['Political instability', 'The spread of pathogens', 'New trade routes', 'Climate change', 'Migrations', 'Tech advancements', 'Mongol invasions', 'Large-scale famine', 'Economic prosperity', 'Religious conflicts']",
  answer: "B",
  subject: "History",
};

const VISION_RECORD_WITH_QUESTION: Readonly<Record<string, unknown>> = {
  id: "test_Math_5",
  image: { src: "https://example.com/math.png", height: 400, width: 600 },
  question: "What is shown in this graph?",
  options: "['Linear', 'Quadratic', 'Exponential']",
  answer: "C",
  subject: "Math",
  topic_difficulty: "Hard",
};

const VISION_RECORD_NO_IMAGE: Readonly<Record<string, unknown>> = {
  id: "test_CS_1",
  image: null,
  options: "['O(n)', 'O(n^2)']",
  answer: "A",
  subject: "Computer_Science",
};
describe("mmmuProVisionRecordToSample", () => {
  it("builds a 10-option MCQ prompt with letters A–J", () => {
    const sample = mmmuProVisionRecordToSample(VISION_RECORD, 0);
    expect(sample.input).toContain("LETTER is one of ABCDEFGHIJ");
    expect(sample.input).toContain("A) Political instability");
    expect(sample.input).toContain("J) Religious conflicts");
  });
  it("uses default question when record has no question field", () => {
    const sample = mmmuProVisionRecordToSample(VISION_RECORD, 0);
    expect(sample.input).toContain("Use the image to answer the question");
  });
  it("uses record question when present", () => {
    const sample = mmmuProVisionRecordToSample(VISION_RECORD_WITH_QUESTION, 0);
    expect(sample.input).toContain("What is shown in this graph?");
    expect(sample.input).not.toContain("Use the image to answer the question");
  });
  it("converts VISION_RECORD to sample with correct id, target, image, and metadata", () => {
    const sample = mmmuProVisionRecordToSample(VISION_RECORD, 0);
    expect(sample.id).toBe("test_History_1");
    expect(sample.target.text).toBe("B");
    expect(sample.contentParts).toBeDefined();
    const imageParts = sample.contentParts!.filter(
      (p) => p.type === "image_url"
    );
    expect(imageParts).toHaveLength(1);
    expect(
      imageParts[0]!.type === "image_url" && imageParts[0]!.imageUrl.url
    ).toBe("https://example.com/history.png");
    expect(sample.metadata?.["question_type"]).toBe("multiple-choice");
    expect(sample.metadata?.["subject"]).toBe("History");
  });
  it("omits contentParts when image is null", () => {
    const sample = mmmuProVisionRecordToSample(VISION_RECORD_NO_IMAGE, 0);
    expect(sample.contentParts).toBeUndefined();
  });
  it("uses fallback id when record id is missing", () => {
    const record = { ...VISION_RECORD, id: undefined };
    const sample = mmmuProVisionRecordToSample(record, 7);
    expect(sample.id).toBe("mmmu-pro-vision-7");
  });
  it("applies imageDetail to image content parts", () => {
    const sample = mmmuProVisionRecordToSample(VISION_RECORD, 0, "low");
    const imageParts = sample.contentParts!.filter(
      (p) => p.type === "image_url"
    );
    expect(imageParts).toHaveLength(1);
    expect(
      imageParts[0]!.type === "image_url" && imageParts[0]!.imageUrl.detail
    ).toBe("low");
  });
  it("omits detail when imageDetail is not provided", () => {
    const sample = mmmuProVisionRecordToSample(VISION_RECORD, 0);
    const imagePart = sample.contentParts!.find((p) => p.type === "image_url");
    expect(
      imagePart!.type === "image_url" && imagePart!.imageUrl.detail
    ).toBeUndefined();
  });
});
describe("MMMU Pro Vision benchmark registry", () => {
  let restoreNetwork: (() => void) | undefined;
  afterEach(() => {
    restoreNetwork?.();
    restoreNetwork = undefined;
  });
  it("resolves mmmu_pro_vision with a complete definition", () => {
    const b = getBenchmark("mmmu_pro_vision");
    expect(b).toBeDefined();
    expect(b?.id).toBe("mmmu_pro_vision");
    expect(b?.temperature).toBe(0);
    expect(b?.defaultEpochs).toBe(1);
    expect(typeof b?.makeLayer).toBe("function");
    expect(typeof b?.makeDatasetLayer).toBe("function");
  });
  it("dispatches runBenchmarkById through the registry entry", async () => {
    restoreNetwork = blockNetwork();
    const result = await runBenchmarkById({
      benchmarkId: "mmmu_pro_vision",
      apiKey: "unused",
      benchmarkConfig: { benchmarkId: "mmmu_pro_vision", model: "test/model" },
      epochs: 1,
      maxConcurrency: 1,
      range: { start: 0, end: 1 },
      datasetRetry: { baseDelayMs: 0 },
      sessionId: "test",
      progressReporter: NOOP_PROGRESS_REPORTER,
    });
    assertFailure(result);
  });
});

describe("mmmuExtraBody", () => {
  it("returns undefined when nothing is configured", () => {
    expect(mmmuExtraBody(undefined, undefined)).toBeUndefined();
  });

  it("forwards the configured extraBody alongside media_resolution", () => {
    expect(
      mmmuExtraBody(
        { deferred_tools: { protocol: "v1" } },
        "MEDIA_RESOLUTION_HIGH"
      )
    ).toEqual({
      deferred_tools: { protocol: "v1" },
      media_resolution: "MEDIA_RESOLUTION_HIGH",
    });
  });

  it("lets the benchmark's media_resolution win over a configured one", () => {
    expect(
      mmmuExtraBody(
        { media_resolution: "MEDIA_RESOLUTION_LOW" },
        "MEDIA_RESOLUTION_HIGH"
      )
    ).toEqual({
      media_resolution: "MEDIA_RESOLUTION_HIGH",
    });
  });
});

describe("mmmuProVisionSolverOptions", () => {
  it("forwards the configured extraBody and media resolution into the solver", () => {
    const parsed = parseSchema(MmmuProVisionBenchmarkConfigSchema, {
      benchmarkId: "mmmu_pro_vision",
      model: "test/model",
      mediaResolution: "MEDIA_RESOLUTION_HIGH",
      extraBody: { deferred_tools: { protocol: "v1" } },
    });
    assertSuccess(parsed);
    const options = mmmuProVisionSolverOptions(parsed.success);
    expect(options.mediaResolution).toBe("MEDIA_RESOLUTION_HIGH");
    expect(options.inference.extraBody).toEqual({
      deferred_tools: { protocol: "v1" },
    });
  });
});
