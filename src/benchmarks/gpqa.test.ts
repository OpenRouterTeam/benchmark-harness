import { describe, expect, it } from "bun:test";

import {
  gen,
  orDie,
  promise,
  provide,
  runPromise,
  succeed,
} from "effect/Effect";
import { HttpClient, HttpClientRequest, HttpClientResponse } from "effect/http";
import { provide as provideLayer, succeed as layerSucceed } from "effect/Layer";
import { fromIterable, runCollect } from "effect/Stream";

import { initialTaskState, MessageRole, ScoreValue } from "../harness/core";
import { Dataset } from "../harness/dataset";
import { Scorer } from "../harness/scorer";
import { decisionsUrl } from "../judge/decisions";
import type { GpqaBenchmarkConfig } from "./benchmark-config";
import { GpqaBenchmarkConfigSchema } from "./benchmark-config";
import {
  gpqaRecordToSample,
  makeGpqaScorerLayer,
  withoutSampleIds,
} from "./gpqa";
import { INTEGRITY_JUDGE_MODEL } from "./scorers/integrity/integrity-spec";

const GPQA_RECORD = {
  Question: "What is 2+2?",
  "Correct Answer": "four",
  "Incorrect Answer 1": "three",
  "Incorrect Answer 2": "five",
  "Incorrect Answer 3": "six",
  Subdomain: "Arithmetic",
} as const;

const OPTION_LINE = /^([ABCD])\) (.+)$/;

function parseOptions(input: string): Record<string, string> {
  const options: Record<string, string> = {};
  for (const line of input.split("\n")) {
    const match = OPTION_LINE.exec(line);
    if (match) {
      options[match[1]!] = match[2]!;
    }
  }
  return options;
}
describe("gpqaRecordToSample", () => {
  it("is deterministic for a given index", () => {
    const a = gpqaRecordToSample(GPQA_RECORD, 7);
    const b = gpqaRecordToSample(GPQA_RECORD, 7);
    expect(a).toEqual(b);
  });
  it("places the correct answer at the target letter", () => {
    const sample = gpqaRecordToSample(GPQA_RECORD, 0);
    const options = parseOptions(sample.input);
    expect(options[sample.target.text]).toBe("four");
  });
  it("includes all four options exactly once", () => {
    const sample = gpqaRecordToSample(GPQA_RECORD, 3);
    const values = Object.values(parseOptions(sample.input)).sort();
    expect(values).toEqual(["five", "four", "six", "three"]);
  });
  it("renders the openbench prompt instruction and question verbatim", () => {
    const sample = gpqaRecordToSample(GPQA_RECORD, 0);
    expect(sample.input).toContain(
      "The last line of your response should be of the following format: 'Answer: $LETTER'"
    );
    expect(sample.input).toContain("What is 2+2?");
  });
  it("preserves `$` special-replacement sequences in question and option text verbatim", () => {
    const dollarRecord = {
      Question: "Evaluate $$x$$ where cost is $5 and ratio is $&y",
      "Correct Answer": "a$$b",
      "Incorrect Answer 1": "c$&d",
      "Incorrect Answer 2": "e$1f",
      "Incorrect Answer 3": "g$`h",
    } as const;
    const sample = gpqaRecordToSample(dollarRecord, 0);
    expect(sample.input).toContain(
      "Evaluate $$x$$ where cost is $5 and ratio is $&y"
    );
    const options = Object.values(parseOptions(sample.input)).sort();
    expect(options).toEqual(["a$$b", "c$&d", "e$1f", "g$`h"].sort());
  });
  it("varies the correct-answer position across records (no fixed bias)", () => {
    const letters = new Set(
      Array.from(
        { length: 20 },
        (_, i) => gpqaRecordToSample(GPQA_RECORD, i).target.text
      )
    );
    expect(letters.size).toBeGreaterThan(1);
  });
  it("derives a stable id and preserves subdomain metadata", () => {
    const sample = gpqaRecordToSample(GPQA_RECORD, 0);
    expect(sample.id).toBe("gpqa_diamond-0");
    expect(sample.metadata?.["subdomain"]).toBe("Arithmetic");
  });
});

describe("makeGpqaScorerLayer", () => {
  it("asks the decisions endpoint and keeps accuracy separate", async () => {
    const requests: { url: string; body: unknown }[] = [];
    const client = HttpClient.make((request) =>
      gen(function* () {
        const web = yield* HttpClientRequest.toWeb(request).pipe(orDie);
        requests.push({
          url: request.url,
          body: yield* promise(() => web.json()),
        });
        return HttpClientResponse.fromWeb(
          request,
          Response.json({
            answers: { level: { probabilities: { coherent_good: 0.1 } } },
          })
        );
      })
    );
    const program = gen(function* () {
      const scorer = yield* Scorer;
      const state = {
        ...initialTaskState({ id: "s1", input: "Q?", target: { text: "B" } }),
        output: {
          completion: "The the answer.\n\nAnswer: B",
          message: {
            role: MessageRole.Assistant,
            content: "The the answer.\n\nAnswer: B",
          },
        },
      };
      return yield* scorer(state, { text: "B" });
    }).pipe(
      provide(
        makeGpqaScorerLayer(
          {
            benchmarkId: "gpqa_diamond",
            integrityRubric: true,
          } as GpqaBenchmarkConfig,
          { apiKey: "k", baseUrl: "https://example.test/api/v1" }
        ).pipe(provideLayer(layerSucceed(HttpClient.HttpClient, client)))
      )
    );
    const score = await runPromise(program);
    expect(score.value).toBe(ScoreValue.Correct);
    expect(requests[0]?.url).toBe("https://example.test/api/alpha/decisions");
    expect(requests[0]?.body).toMatchObject({ model: INTEGRITY_JUDGE_MODEL });
    expect(score.trajectory).toMatchObject({
      kind: "judge_runs",
      runs: [{ judge: "integrity", flagged: true }],
    });
  });
});

describe("decisionsUrl", () => {
  it("derives the decisions endpoint from the API base URL", () => {
    expect(decisionsUrl()).toBe("https://openrouter.ai/api/alpha/decisions");
    expect(decisionsUrl("https://openrouter.ai/api/v1/")).toBe(
      "https://openrouter.ai/api/alpha/decisions"
    );
  });

  it("accepts an origin-only base URL like the model client does", () => {
    expect(decisionsUrl("http://localhost:8787")).toBe(
      "http://localhost:8787/api/alpha/decisions"
    );
  });
});

describe("withoutSampleIds", () => {
  const samples = Array.from({ length: 6 }, (_, i) =>
    gpqaRecordToSample(GPQA_RECORD, i)
  );
  const source = layerSucceed(
    Dataset,
    Dataset.of({
      stream: () => fromIterable(samples),
      size: succeed(samples.length),
    })
  );
  const filtered = withoutSampleIds(source, [
    "gpqa_diamond-1",
    "gpqa_diamond-4",
  ]);
  const collectIds = (opts?: { start?: number; end?: number }) =>
    runPromise(
      gen(function* () {
        const dataset = yield* Dataset;
        const chunk = yield* runCollect(dataset.stream(opts));
        return { size: yield* dataset.size, ids: chunk.map((s) => s.id) };
      }).pipe(provide(filtered))
    );

  it("drops the listed ids and reports the reduced size", async () => {
    expect(await collectIds()).toEqual({
      size: 4,
      ids: [
        "gpqa_diamond-0",
        "gpqa_diamond-2",
        "gpqa_diamond-3",
        "gpqa_diamond-5",
      ],
    });
  });

  it("slices start/end over the kept samples", async () => {
    const { ids } = await collectIds({ start: 1, end: 3 });
    expect(ids).toEqual(["gpqa_diamond-2", "gpqa_diamond-3"]);
  });

  it("rejects ids outside the gpqa_diamond-<index> form", () => {
    const parsed = GpqaBenchmarkConfigSchema.shape.excludeSampleIds.safeParse([
      "mmlu-1",
    ]);
    expect(parsed.success).toBe(false);
  });
});
