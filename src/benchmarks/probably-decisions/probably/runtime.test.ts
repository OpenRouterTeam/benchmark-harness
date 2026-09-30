import { describe, expect, it } from "bun:test";

import { probablyProgramSource } from "../programs";
import type { Provider } from "./runtime";
import { run } from "./runtime";

function keywordProvider(picks: Readonly<Record<string, string>>): Provider {
  return {
    write: (prompt) => Promise.resolve(`wrote:${prompt}`),
    judge: (_value, labels) => {
      const hit = Object.entries(picks).find(([needle]) =>
        labels.some((label) => label.includes(needle))
      );
      const chosen =
        hit === undefined ? labels[0] : labels.find((l) => l.includes(hit[0]));
      return Promise.resolve(
        Object.fromEntries(
          labels.map((label) => [
            label,
            label === chosen ? 0.95 : 0.05 / (labels.length - 1),
          ])
        )
      );
    },
  };
}

describe("probably runtime", () => {
  it("runs if/else, match, repeat and print with a recorded tape", async () => {
    const source = [
      "let mood = input()",
      'if mood feels "happy" with confidence 60% {',
      '  print("yes")',
      "} otherwise maybe {",
      '  print("maybe")',
      "} else {",
      '  print("no")',
      "}",
      "match mood {",
      '  "calm" => {',
      '    print("calm")',
      "  }",
      '  "angry" => {',
      '    print("angry")',
      "  }",
      "}",
    ].join("\n");
    const result = await run(source, keywordProvider({ happy: "", calm: "" }), {
      input: "sunny day",
    });
    expect(result.output).toEqual(["yes", "calm"]);
    expect(result.tape).toHaveLength(2);
    expect(result.trace.filter((e) => e.kind === "judge")).toHaveLength(2);
  });

  it("replays a tape without calling the provider", async () => {
    const source =
      'let x = input()\nif x feels "ok" {\n  print("a")\n} else {\n  print("b")\n}';
    const recorded = await run(source, keywordProvider({ ok: "" }), {
      input: "fine",
    });
    const failing: Provider = {
      write: () => Promise.reject(new Error("no")),
      judge: () => Promise.reject(new Error("no")),
    };
    const replayed = await run(source, failing, {
      input: "fine",
      replay: recorded.tape,
    });
    expect(replayed.output).toEqual(["a"]);
  });

  it("rejects a replay tape from a different input", async () => {
    const source = 'let x = input()\nif x feels "ok" {\n  print("a")\n}';
    const recorded = await run(source, keywordProvider({ ok: "" }), {
      input: "fine",
    });
    await expect(
      run(source, keywordProvider({}), {
        input: "other",
        replay: recorded.tape,
      })
    ).rejects.toThrow("Replay does not match");
  });

  it("falls to the maybe branch when confidence is under the threshold", async () => {
    const flat: Provider = {
      write: () => Promise.reject(new Error("no")),
      judge: (_v, labels) =>
        Promise.resolve(
          Object.fromEntries(labels.map((l) => [l, 1 / labels.length]))
        ),
    };
    const source =
      'let x = input()\nif x feels "ok" with confidence 80% {\n  print("a")\n} otherwise maybe {\n  print("m")\n} else {\n  print("b")\n}';
    const result = await run(source, flat, { input: "?" });
    expect(result.output).toEqual(["m"]);
  });

  it("stops at the model-call limit", async () => {
    const source =
      'let x = input()\nrepeat 5 {\n  if x feels "ok" {\n    print("a")\n  }\n}';
    await expect(
      run(source, keywordProvider({ ok: "" }), { input: "fine", maxEffects: 3 })
    ).rejects.toThrow("model-call limit");
  });
});

describe("sentinel_case_v1 program", () => {
  const source = probablyProgramSource("sentinel_case_v1");

  it("holds when the compromised-key gate is uncertain", async () => {
    const flat: Provider = {
      write: () => Promise.reject(new Error("no")),
      judge: (_v, labels) =>
        Promise.resolve(
          Object.fromEntries(labels.map((l) => [l, 1 / labels.length]))
        ),
    };
    const result = await run(source, flat, { input: "dossier" });
    expect(result.output.at(-1)).toBe("hold");
  });

  it("revokes keys when the judge is confident the key is compromised", async () => {
    const result = await run(
      source,
      keywordProvider({ "leaked, harvested": "" }),
      {
        input: "dossier",
      }
    );
    expect(result.output.at(-1)).toBe("key_revocation");
  });

  it("holds on static-only attributes", async () => {
    const result = await run(
      source,
      keywordProvider({ "NOT: the traffic": "", static: "" }),
      { input: "dossier" }
    );
    expect(result.output.at(-1)).toBe("hold");
  });

  it("proposes a frontier block for corroborated coordinated abuse", async () => {
    const result = await run(
      source,
      keywordProvider({
        "NOT: the traffic": "",
        "acting together": "",
        corroborated: "",
        "frontier US models": "",
      }),
      { input: "dossier" }
    );
    expect(result.output.at(-1)).toBe("frontier_block");
  });
});
