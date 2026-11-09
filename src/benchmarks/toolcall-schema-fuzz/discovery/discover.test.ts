import { describe, expect, it } from "bun:test";

import type { Probe, Realize } from "./discover";
import { discover, findingSignature, PASS, TRANSPORT } from "./discover";

const SEED = {
  name: "fuzz",
  strict: false,
  parameters: {
    type: "object",
    properties: {
      a: { $ref: "#/$defs/n" },
      b: { type: "string" },
    },
    required: ["a", "b"],
    $defs: { n: { type: "integer" } },
  },
  args: { a: 5, b: "hello" },
};

const refBug: Probe = async (entry, provider) => {
  const schema = JSON.stringify(entry.tools[0]?.function.parameters);
  const failing =
    provider === "bad" &&
    schema.includes('"$ref"') &&
    schema.includes('"integer"');
  return failing
    ? {
        category: "schema_violation",
        explanation: 'schema_violation: $.a expected integer, got string "5"',
      }
    : { category: PASS, explanation: "" };
};

const WORDING = {
  description: "Look up a shipment by its tracking number.",
  prompts: {
    first: "Where is shipment 5?",
    second: "Where is shipment 5?",
    both: "Where is shipment 5?",
  },
  distractor: { name: "cancel_shipment", description: "Cancel a shipment." },
};

const realistic: Realize = async (candidate) => ({
  ...candidate,
  name: "track_shipment",
  wording: WORDING,
});

const syntheticOnly: Probe = async (entry, provider) =>
  entry.tools.some((tool) => tool.function.name === "track_shipment")
    ? { category: PASS, explanation: "" }
    : refBug(entry, provider);

const CONFIG = {
  model: "test/model",
  targets: ["good", "bad"],
  reference: "good",
  repeats: 2,
  budget: 500,
  known: new Set<string>(),
  now: "2026-10-05T00:00:00.000Z",
};

describe("discover", () => {
  it("shrinks a confirmed failure, pairs it with a passing control and checks the reference", async () => {
    const findings = await discover([SEED], {
      config: CONFIG,
      probe: refBug,
      realize: realistic,
    });
    expect(findings).toHaveLength(1);
    const [finding] = findings;
    expect(finding?.finding).toEqual({
      category: "schema_violation",
      explanation: 'schema_violation: $.a expected integer, got string "5"',
      model: "test/model",
      provider: "bad",
      observed: { failures: 2, runs: 2 },
      reference: { provider: "good", observed: { failures: 0, runs: 2 } },
      discoveredAt: "2026-10-05T00:00:00.000Z",
    });
    expect(finding?.failing.parameters).toEqual({
      type: "object",
      properties: { a: { $ref: "#/$defs/n" } },
      required: ["a"],
      $defs: { n: { type: "integer" } },
    });
    expect(finding?.failing.args).toEqual({ a: 0 });
    expect(finding?.failing.name).toBe("track_shipment");
    expect(finding?.failing.wording).toEqual(WORDING);
    expect(finding?.control?.wording).toEqual(WORDING);
    expect(JSON.stringify(finding?.control?.parameters)).not.toContain("$ref");
  });

  it("skips failures already on file and respects the budget", async () => {
    const first = await discover([SEED], {
      config: CONFIG,
      probe: refBug,
      realize: realistic,
    });
    const [finding] = first;
    const known = new Set(
      finding === undefined
        ? []
        : [
            findingSignature({
              provider: "bad",
              category: finding.finding.category,
              parameters: finding.failing.parameters,
            }),
          ]
    );
    expect(
      await discover([SEED], {
        config: { ...CONFIG, known },
        probe: refBug,
        realize: realistic,
      })
    ).toEqual([]);
    expect(
      await discover([SEED], {
        config: { ...CONFIG, budget: 0 },
        probe: refBug,
        realize: realistic,
      })
    ).toEqual([]);
  });

  it("does not confirm a failure that passes on some repeats", async () => {
    let calls = 0;
    const flaky: Probe = async (entry, provider) => {
      calls += 1;
      return calls % 2 === 0
        ? { category: PASS, explanation: "" }
        : refBug(entry, provider);
    };
    expect(
      await discover([SEED], {
        config: { ...CONFIG, targets: ["bad"] },
        probe: flaky,
        realize: realistic,
      })
    ).toEqual([]);
  });

  it("drops a finding whose final check ran out of budget", async () => {
    const config = { ...CONFIG, targets: ["bad"], repeats: 1, budget: 2 };
    expect(
      await discover([SEED], { config, probe: refBug, realize: realistic })
    ).toEqual([]);
  });

  it("does not report transport errors as findings", async () => {
    const flaky: Probe = async () => ({
      category: TRANSPORT,
      explanation: "HTTP 429",
    });
    expect(
      await discover([SEED], {
        config: CONFIG,
        probe: flaky,
        realize: realistic,
      })
    ).toEqual([]);
  });

  it("drops a failure that does not reproduce under its realistic rewording", async () => {
    expect(
      await discover([SEED], {
        config: CONFIG,
        probe: syntheticOnly,
        realize: realistic,
      })
    ).toEqual([]);
  });

  it("drops a failure with no accepted rewording", async () => {
    const none: Realize = async () => undefined;
    expect(
      await discover([SEED], { config: CONFIG, probe: refBug, realize: none })
    ).toEqual([]);
  });
});
