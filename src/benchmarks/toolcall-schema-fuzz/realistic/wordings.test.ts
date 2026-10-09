import { describe, expect, it } from "bun:test";

import { gridShapeDraws } from "../grid";
import { FuzzScenario } from "../tool-case";
import type { WordingEntry } from "./wordings";
import {
  realisticCases,
  TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES,
  unwordedShapes,
} from "./wordings";

const ENTRY = {
  key: "integer",
  name: "start_countdown_timer",
  description: "Start a countdown timer.",
  parameters: {
    type: "object",
    properties: { duration_minutes: { type: "integer" } },
    required: ["duration_minutes"],
    additionalProperties: false,
  },
  args: [{ duration_minutes: 25 }, { duration_minutes: 10 }],
  prompts: {
    first: "Start a 25 minute timer.",
    second: "Start a 10 minute timer.",
    both: "Start one timer for 25 minutes and another for 10.",
  },
  distractor: { name: "cancel_timer", description: "Cancel a running timer." },
} satisfies WordingEntry;

const FILE = { version: 1, writer: "w", checker: "c", entries: [ENTRY] };

describe("realisticCases", () => {
  const cases = realisticCases(FILE);

  it("runs a reworded shape in every grid scenario under its realistic tool and prompts", () => {
    expect(cases.map((entry) => entry.id)).toEqual([
      "toolcall_schema_fuzz-realistic:loose:integer",
      "toolcall_schema_fuzz-realistic:strict:integer",
      "toolcall_schema_fuzz-realistic:loose:replay:integer",
      "toolcall_schema_fuzz-realistic:strict:replay:integer",
      "toolcall_schema_fuzz-realistic:loose:distractor:integer",
      "toolcall_schema_fuzz-realistic:strict:distractor:integer",
      "toolcall_schema_fuzz-realistic:loose:parallel:integer",
      "toolcall_schema_fuzz-realistic:strict:parallel:integer",
    ]);
    const distractor = cases.find(
      (entry) => entry.scenario === FuzzScenario.Distractor
    );
    expect(distractor?.tools.map((tool) => tool.function.name)).toEqual([
      "cancel_timer",
      "start_countdown_timer",
    ]);
    expect(distractor?.messages[0]).toEqual({
      role: "user",
      content: ENTRY.prompts.first,
    });
    const parallel = cases.find(
      (entry) => entry.scenario === FuzzScenario.Parallel
    );
    expect(parallel?.messages[0]).toEqual({
      role: "user",
      content: ENTRY.prompts.both,
    });
    expect(parallel?.calls.map((call) => call.args)).toEqual([...ENTRY.args]);
  });

  it("puts the expected call in the history and asks the model to repeat it", () => {
    const single = cases.find(
      (entry) => entry.scenario === FuzzScenario.Single
    );
    expect(single?.messages).toEqual([
      { role: "user", content: ENTRY.prompts.first },
      {
        role: "assistant",
        content: "",
        tool_calls: [
          {
            id: "call_0",
            type: "function",
            function: {
              name: ENTRY.name,
              arguments: JSON.stringify(ENTRY.args[0]),
            },
          },
        ],
      },
      { role: "tool", tool_call_id: "call_0", content: '{"ok":true}' },
      { role: "user", content: single?.prompt },
    ]);
    expect(single?.prompt).toContain("again with exactly the same arguments");
    expect(single?.calls).toEqual([{ name: ENTRY.name, args: ENTRY.args[0] }]);
  });

  it("repeats the latest call after an earlier tool turn", () => {
    const replay = cases.find(
      (entry) => entry.scenario === FuzzScenario.Replay
    );
    const ids = replay?.messages.flatMap((message) =>
      message.role === "assistant"
        ? message.tool_calls.map((call) => call.id)
        : []
    );
    expect(ids).toEqual(["call_0", "call_1"]);
    expect(replay?.calls.map((call) => call.args)).toEqual([ENTRY.args[0]]);
  });

  it("replays both parallel calls in one turn and asks for both again", () => {
    const parallel = cases.find(
      (entry) => entry.scenario === FuzzScenario.Parallel
    );
    const turn = parallel?.messages[1];
    expect(
      turn?.role === "assistant"
        ? turn.tool_calls.map((call) => call.function.arguments)
        : []
    ).toEqual(ENTRY.args.map((args) => JSON.stringify(args)));
    expect(
      parallel?.messages.filter((message) => message.role === "tool")
    ).toHaveLength(2);
    expect(parallel?.prompt).toContain(
      "Make the same 2 `start_countdown_timer` calls again"
    );
  });

  it("links no controls, since each shape is reworded as a different task", () => {
    expect(cases.map((entry) => entry.control)).toEqual(
      cases.map(() => undefined)
    );
    expect(cases.map((entry) => entry.index)).toEqual(
      cases.map((_, index) => index)
    );
  });

  it("skips parallel calls when there is no request for both", () => {
    const { both: _both, ...prompts } = ENTRY.prompts;
    const scenarios = realisticCases({
      ...FILE,
      entries: [{ ...ENTRY, prompts }],
    }).map((entry) => entry.scenario);
    expect(scenarios).toHaveLength(6);
    expect(scenarios).not.toContain(FuzzScenario.Parallel);
  });

  it("skips parallel calls when both draws are the same", () => {
    const same = {
      ...ENTRY,
      args: [ENTRY.args[0], ENTRY.args[0]],
    } satisfies WordingEntry;
    const scenarios = realisticCases({ ...FILE, entries: [same] }).map(
      (entry) => entry.scenario
    );
    expect(scenarios).not.toContain(FuzzScenario.Parallel);
  });
});

describe("unwordedShapes", () => {
  it("lists every grid shape not yet reworded", () => {
    const all = gridShapeDraws();
    const remaining = unwordedShapes(FILE);
    expect(remaining).toHaveLength(all.length - 1);
    expect(remaining.map(({ shape }) => shape.key)).not.toContain("integer");
  });
});

describe("TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES", () => {
  it("matches the catalog dataset size", () => {
    expect(TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES).toHaveLength(2668);
  });
});
