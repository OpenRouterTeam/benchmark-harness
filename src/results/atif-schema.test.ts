import { describe, expect, it } from "bun:test";

import { assertLeft, assertRight } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import {
  AtifAudioSourceSchema,
  AtifContentPartSchema,
  AtifStepSchema,
  AtifTrajectorySchema,
  AtifSubagentTrajectoryRefSchema,
} from "./atif-schema";

const VALID_TRAJECTORY = {
  schema_version: "ATIF-v1.8",
  session_id: "session-1",
  trajectory_id: "trajectory-1",
  agent: {
    name: "openrouter-bench",
    version: "2",
    model_name: "openai/gpt-4o-mini",
    tool_definitions: [{ name: "lookup" }],
  },
  steps: [
    {
      step_id: 1,
      source: "user",
      message: [
        { type: "text", text: "Find the answer" },
        {
          type: "image",
          source: {
            media_type: "image/png",
            path: "data:image/png;base64,abc",
          },
        },
      ],
    },
    {
      step_id: 2,
      source: "agent",
      model_name: "openai/gpt-4o-mini",
      reasoning_content: "I should use the lookup tool.",
      tool_calls: [
        {
          tool_call_id: "call-1",
          function_name: "lookup",
          arguments: { query: "answer" },
        },
      ],
      metrics: {
        prompt_tokens: 10,
        completion_tokens: 5,
        cost_usd: 0.01,
      },
      message: "I will look that up.",
      observation: {
        results: [
          {
            source_call_id: "call-1",
            content: "The answer is 42.",
            subagent_trajectory_ref: [
              {
                trajectory_id: "sub-trajectory-1",
                session_id: "sub-session-1",
                trajectory_path: "subagent.json",
              },
            ],
          },
        ],
      },
    },
    {
      step_id: 3,
      source: "agent",
      model_name: "openai/gpt-4o-mini",
      message: "The answer is 42.",
    },
  ],
  final_metrics: {
    total_prompt_tokens: 10,
    total_completion_tokens: 5,
    total_steps: 3,
  },
  subagent_trajectories: [
    {
      schema_version: "ATIF-v1.8",
      trajectory_id: "sub-trajectory-1",
      agent: { name: "subagent", version: "1" },
      steps: [{ step_id: 1, source: "agent", message: "Done." }],
    },
  ],
} as const;

describe("AtifTrajectorySchema", () => {
  it("parses a valid multimodal trajectory with tools and a subagent", () => {
    const parsed = parseSchema(AtifTrajectorySchema, VALID_TRAJECTORY);
    assertRight(parsed);
    expect(parsed.right).toEqual(VALID_TRAJECTORY);
  });

  it("rejects unknown top-level keys", () => {
    const parsed = parseSchema(AtifTrajectorySchema, {
      ...VALID_TRAJECTORY,
      unexpected: true,
    });
    assertLeft(parsed);
  });

  it("rejects non-sequential step ids", () => {
    const parsed = parseSchema(AtifTrajectorySchema, {
      ...VALID_TRAJECTORY,
      steps: [
        VALID_TRAJECTORY.steps[0],
        { ...VALID_TRAJECTORY.steps[1], step_id: 3 },
        VALID_TRAJECTORY.steps[2],
      ],
    });
    assertLeft(parsed);
  });

  it("rejects observations with an unknown source call id", () => {
    const parsed = parseSchema(AtifTrajectorySchema, {
      ...VALID_TRAJECTORY,
      steps: [
        VALID_TRAJECTORY.steps[0],
        {
          ...VALID_TRAJECTORY.steps[1],
          observation: {
            results: [{ source_call_id: "missing", content: "result" }],
          },
        },
        VALID_TRAJECTORY.steps[2],
      ],
    });
    assertLeft(parsed);
  });

  it("rejects tool calls on user steps", () => {
    const parsed = parseSchema(AtifTrajectorySchema, {
      ...VALID_TRAJECTORY,
      steps: [
        {
          ...VALID_TRAJECTORY.steps[0],
          tool_calls: [
            {
              tool_call_id: "call-1",
              function_name: "lookup",
              arguments: {},
            },
          ],
        },
        VALID_TRAJECTORY.steps[1],
        VALID_TRAJECTORY.steps[2],
      ],
    });
    assertLeft(parsed);
  });

  it("rejects duplicate subagent trajectory ids", () => {
    const parsed = parseSchema(AtifTrajectorySchema, {
      ...VALID_TRAJECTORY,
      subagent_trajectories: [
        VALID_TRAJECTORY.subagent_trajectories[0],
        VALID_TRAJECTORY.subagent_trajectories[0],
      ],
    });
    assertLeft(parsed);
  });

  it("rejects an embedded subagent without a trajectory id", () => {
    const parsed = parseSchema(AtifTrajectorySchema, {
      ...VALID_TRAJECTORY,
      subagent_trajectories: [
        {
          ...VALID_TRAJECTORY.subagent_trajectories[0],
          trajectory_id: undefined,
        },
      ],
    });
    assertLeft(parsed);
  });

  it("rejects a reference without an id or path", () => {
    const parsed = parseSchema(AtifSubagentTrajectoryRefSchema, {
      session_id: "session-1",
    });
    assertLeft(parsed);
  });

  it("rejects content parts missing text", () => {
    const parsed = parseSchema(AtifContentPartSchema, { type: "text" });
    assertLeft(parsed);
  });

  it("normalizes Harbor audio media type aliases", () => {
    const mp3 = parseSchema(AtifAudioSourceSchema, {
      media_type: "audio/mp3",
      path: "audio.mp3",
    });
    assertRight(mp3);
    expect(mp3.right.media_type).toBe("audio/mpeg");

    const wav = parseSchema(AtifAudioSourceSchema, {
      media_type: " AUDIO/X-WAV ",
      path: "audio.wav",
    });
    assertRight(wav);
    expect(wav.right.media_type).toBe("audio/wav");
  });

  it("rejects unsupported audio media types", () => {
    const parsed = parseSchema(AtifAudioSourceSchema, {
      media_type: "audio/midi",
      path: "audio.mid",
    });
    assertLeft(parsed);
  });

  it("accepts Harbor ISO 8601 timestamps", () => {
    for (const timestamp of [
      "2026-09-22T01:12:00Z",
      "2026-09-22T01:12:00.123+05:30",
      "2026-09-22",
      "2028-02-29T00:00:00+00:00",
    ]) {
      const parsed = parseSchema(AtifStepSchema, {
        step_id: 1,
        source: "user",
        message: "hello",
        timestamp,
      });
      assertRight(parsed);
    }
  });

  it("rejects invalid Harbor ISO 8601 timestamps", () => {
    for (const timestamp of [
      "yesterday",
      "2026-13-45T00:00:00Z",
      "2026-02-30",
      "2027-02-29T00:00:00Z",
      "2026-04-31",
      "2026-09-22T24:00:00Z",
    ]) {
      const parsed = parseSchema(AtifStepSchema, {
        step_id: 1,
        source: "user",
        message: "hello",
        timestamp,
      });
      assertLeft(parsed);
    }
  });
});
