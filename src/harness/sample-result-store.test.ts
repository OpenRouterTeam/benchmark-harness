import { describe, expect, it } from "bun:test";

import { ScoreValue } from "./core";
import type {
  PersistedSampleOutcome,
  SampleResultStoreService,
} from "./sample-result-store";
import {
  namespacedSampleResultStore,
  sampleResultKey,
} from "./sample-result-store";

const OUTCOME: PersistedSampleOutcome = {
  sampleScore: {
    sampleId: "s1",
    epoch: 0,
    score: { value: ScoreValue.Correct, answer: "B", explanation: "" },
  },
};

describe("namespacedSampleResultStore", () => {
  it("prefixes reads and writes so different sessions do not collide", async () => {
    const map = new Map<string, PersistedSampleOutcome>();
    const backing: SampleResultStoreService = {
      read: async (key) => map.get(key) ?? null,
      write: async (key, data) => {
        map.set(key, data);
      },
    };
    const sessionA = namespacedSampleResultStore("session-a", backing);
    const sessionB = namespacedSampleResultStore("session-b", backing);
    const key = sampleResultKey("s1", 0);

    await sessionA.write(key, OUTCOME);

    expect([...map.keys()]).toEqual(["session-a/s1/0"]);
    expect(await sessionA.read(key)).toEqual(OUTCOME);
    expect(await sessionB.read(key)).toBeNull();
  });
});
