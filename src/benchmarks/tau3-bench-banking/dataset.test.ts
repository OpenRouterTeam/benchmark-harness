import { afterEach, beforeEach, describe, expect, it } from "bun:test";

import { flatMap, provide, runPromise } from "effect/Effect";

import { Dataset } from "../../harness/dataset";
import { bankingRecordToSample, makeBankingDatasetLayer } from "./dataset";
import {
  makeEmptyBankingData,
  seedBankingCache,
  seedBankingTasksRawCache,
} from "./environment";
import type { BankingData, Tau3Task } from "./types";
describe("tau3-bench-banking dataset", () => {
  let originalFetch: typeof global.fetch;
  const fixtureDb: BankingData = {
    ...makeEmptyBankingData(),
    users: { data: { "1": { name: "Test" } } },
  };
  const fixtureTasks: Tau3Task[] = [
    {
      id: "task_001",
      description: null,
      user_scenario: { instructions: "First task instructions" },
      initial_state: null,
      evaluation_criteria: null,
      annotations: null,
    },
    {
      id: "task_002",
      description: null,
      user_scenario: { instructions: "Second task instructions" },
      initial_state: null,
      evaluation_criteria: null,
      annotations: null,
    },
    {
      id: "task_003",
      description: null,
      user_scenario: { instructions: "Third task instructions" },
      initial_state: null,
      evaluation_criteria: null,
      annotations: null,
    },
  ];
  beforeEach(() => {
    originalFetch = global.fetch;
    seedBankingCache(fixtureDb, fixtureTasks);
  });
  afterEach(() => {
    global.fetch = originalFetch;
  });
  describe("bankingRecordToSample", () => {
    it("converts tasks to samples with correct id, instructions input, and empty target", () => {
      const sample1 = bankingRecordToSample(fixtureTasks[0]!);
      const sample2 = bankingRecordToSample(fixtureTasks[1]!);
      expect(sample1.id).toBe("tau3_bench_banking-task_001");
      expect(sample1.input).toBe("First task instructions");
      expect(sample1.target.text).toBe("");
      expect(sample2.input).toBe("Second task instructions");
    });
    it("includes task in metadata", () => {
      const sample = bankingRecordToSample(fixtureTasks[0]!);
      expect(sample.metadata?.task).toBe(fixtureTasks[0]);
    });
    it("generates unique ids for different tasks", () => {
      const sample1 = bankingRecordToSample(fixtureTasks[0]!);
      const sample2 = bankingRecordToSample(fixtureTasks[1]!);
      expect(sample1.id).not.toBe(sample2.id);
      expect(sample1.id).toBe("tau3_bench_banking-task_001");
      expect(sample2.id).toBe("tau3_bench_banking-task_002");
    });
  });
  describe("makeBankingDatasetLayer", () => {
    it("loads pinned tasks before resolving dataset size", async () => {
      seedBankingTasksRawCache("");
      global.fetch = async () => Response.json(fixtureTasks);
      const layer = makeBankingDatasetLayer();
      const size = await runPromise(
        Dataset.pipe(
          flatMap((dataset) => dataset.size),
          provide(layer)
        )
      );
      expect(size).toBe(fixtureTasks.length);
    });
    it("retries a transient fetch failure per the retry config", async () => {
      seedBankingTasksRawCache("");
      let fetchCalls = 0;
      global.fetch = async () => {
        fetchCalls++;
        if (fetchCalls === 1) {
          return new Response("upstream hiccup", { status: 503 });
        }
        return Response.json(fixtureTasks);
      };
      const layer = makeBankingDatasetLayer({ baseDelayMs: 0, maxRetries: 1 });
      const size = await runPromise(
        Dataset.pipe(
          flatMap((dataset) => dataset.size),
          provide(layer)
        )
      );
      expect(size).toBe(fixtureTasks.length);
      expect(fetchCalls).toBe(2);
    });
  });
});
