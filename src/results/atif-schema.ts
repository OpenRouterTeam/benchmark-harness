import { z } from "../internal/zod";

export const AtifImageSourceSchema = z.strictObject({
  media_type: z.enum(["image/jpeg", "image/png", "image/gif", "image/webp"]),
  path: z.string(),
});

export type AtifImageSource = z.infer<typeof AtifImageSourceSchema>;

const AUDIO_MEDIA_TYPE_ALIASES: Readonly<Record<string, string>> = {
  "audio/mp3": "audio/mpeg",
  "audio/mpga": "audio/mpeg",
  "audio/x-mpeg": "audio/mpeg",
  "audio/x-wav": "audio/wav",
  "audio/wave": "audio/wav",
  "audio/vnd.wave": "audio/wav",
  "audio/x-m4a": "audio/mp4",
  "audio/m4a": "audio/mp4",
  "audio/x-aac": "audio/aac",
  "audio/x-flac": "audio/flac",
  "audio/x-aiff": "audio/aiff",
};

export const AtifAudioSourceSchema = z.strictObject({
  media_type: z.preprocess(
    (value: unknown) => {
      if (typeof value !== "string") {
        return value;
      }
      const normalized = value.trim().toLowerCase();
      return AUDIO_MEDIA_TYPE_ALIASES[normalized] ?? normalized;
    },
    z.enum([
      "audio/wav",
      "audio/mpeg",
      "audio/mp4",
      "audio/aac",
      "audio/ogg",
      "audio/flac",
      "audio/webm",
      "audio/aiff",
    ])
  ),
  path: z.string(),
  duration_sec: z.number().min(0).optional(),
});

export type AtifAudioSource = z.infer<typeof AtifAudioSourceSchema>;

export const AtifContentPartSchema = z.discriminatedUnion("type", [
  z.strictObject({ type: z.literal("text"), text: z.string() }),
  z.strictObject({
    type: z.literal("image"),
    source: AtifImageSourceSchema,
  }),
  z.strictObject({
    type: z.literal("audio"),
    source: AtifAudioSourceSchema,
  }),
]);

export type AtifContentPart = z.infer<typeof AtifContentPartSchema>;

export const AtifToolCallSchema = z.strictObject({
  tool_call_id: z.string(),
  function_name: z.string(),
  arguments: z.record(z.string(), z.unknown()),
  extra: z.record(z.string(), z.unknown()).optional(),
});

export type AtifToolCall = z.infer<typeof AtifToolCallSchema>;

export const AtifSubagentTrajectoryRefSchema = z
  .strictObject({
    trajectory_id: z.string().optional(),
    session_id: z.string().optional(),
    trajectory_path: z.string().optional(),
    extra: z.record(z.string(), z.unknown()).optional(),
  })
  .refine(
    (ref) =>
      ref.trajectory_id !== undefined || ref.trajectory_path !== undefined,
    { message: "trajectory_id or trajectory_path is required" }
  );

export type AtifSubagentTrajectoryRef = z.infer<
  typeof AtifSubagentTrajectoryRefSchema
>;

export const AtifObservationResultSchema = z.strictObject({
  source_call_id: z.string().optional(),
  content: z.union([z.string(), z.array(AtifContentPartSchema)]).optional(),
  subagent_trajectory_ref: z.array(AtifSubagentTrajectoryRefSchema).optional(),
  extra: z.record(z.string(), z.unknown()).optional(),
});

export type AtifObservationResult = z.infer<typeof AtifObservationResultSchema>;

export const AtifObservationSchema = z.strictObject({
  results: z.array(AtifObservationResultSchema),
});

export type AtifObservation = z.infer<typeof AtifObservationSchema>;

export const AtifMetricsSchema = z.strictObject({
  prompt_tokens: z.number().int().optional(),
  completion_tokens: z.number().int().optional(),
  cached_tokens: z.number().int().optional(),
  cost_usd: z.number().optional(),
  prompt_token_ids: z.array(z.number().int()).optional(),
  completion_token_ids: z.array(z.number().int()).optional(),
  logprobs: z.array(z.number()).optional(),
  extra: z.record(z.string(), z.unknown()).optional(),
});

export type AtifMetrics = z.infer<typeof AtifMetricsSchema>;

export const AtifFinalMetricsSchema = z.strictObject({
  total_prompt_tokens: z.number().int().optional(),
  total_completion_tokens: z.number().int().optional(),
  total_cached_tokens: z.number().int().optional(),
  total_cost_usd: z.number().optional(),
  total_steps: z.number().int().min(0).optional(),
  extra: z.record(z.string(), z.unknown()).optional(),
});

export type AtifFinalMetrics = z.infer<typeof AtifFinalMetricsSchema>;

export const AtifAgentSchema = z.strictObject({
  name: z.string(),
  version: z.string(),
  model_name: z.string().optional(),
  tool_definitions: z.array(z.record(z.string(), z.unknown())).optional(),
  extra: z.record(z.string(), z.unknown()).optional(),
});

export type AtifAgent = z.infer<typeof AtifAgentSchema>;

export const ATIF_STEP_SOURCES = ["system", "user", "agent"] as const;

export type AtifStepSource = (typeof ATIF_STEP_SOURCES)[number];

export const AtifStepSourceSchema = z.enum(ATIF_STEP_SOURCES);

const ISO_8601_TIMESTAMP_PATTERN =
  /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(?:Z|([+-])(\d{2}):?(\d{2})|([+-])(\d{2}))?)?$/;

function isIso8601Timestamp(value: string): boolean {
  const match = ISO_8601_TIMESTAMP_PATTERN.exec(value);
  if (match === null) {
    return false;
  }
  const [
    ,
    year,
    month,
    day,
    hour,
    minute,
    second,
    ,
    offsetHour,
    offsetMinute,
    ,
    shortOffsetHour,
  ] = match.map((group) => (group === undefined ? undefined : Number(group)));
  if (year === undefined || month === undefined || day === undefined) {
    return false;
  }
  const daysInMonth = new Date(Date.UTC(year, month, 0)).getUTCDate();
  return (
    month >= 1 &&
    month <= 12 &&
    day >= 1 &&
    day <= daysInMonth &&
    (hour ?? 0) <= 23 &&
    (minute ?? 0) <= 59 &&
    (second ?? 0) <= 59 &&
    (offsetHour ?? shortOffsetHour ?? 0) <= 23 &&
    (offsetMinute ?? 0) <= 59
  );
}

export const AtifStepSchema = z
  .strictObject({
    step_id: z.number().int().min(1),
    timestamp: z
      .string()
      .refine(isIso8601Timestamp, { message: "timestamp must be ISO 8601" })
      .optional(),
    source: AtifStepSourceSchema,
    model_name: z.string().optional(),
    reasoning_effort: z.union([z.string(), z.number()]).optional(),
    message: z.union([z.string(), z.array(AtifContentPartSchema)]),
    reasoning_content: z.string().optional(),
    tool_calls: z.array(AtifToolCallSchema).optional(),
    observation: AtifObservationSchema.optional(),
    metrics: AtifMetricsSchema.optional(),
    is_copied_context: z.boolean().optional(),
    llm_call_count: z.number().int().min(0).optional(),
    extra: z.record(z.string(), z.unknown()).optional(),
  })
  .superRefine((step, ctx) => {
    if (step.source !== "agent") {
      const fields = [
        ["model_name", step.model_name],
        ["reasoning_effort", step.reasoning_effort],
        ["reasoning_content", step.reasoning_content],
        ["tool_calls", step.tool_calls],
        ["metrics", step.metrics],
      ] as const;
      for (const [field, value] of fields) {
        if (value !== undefined) {
          ctx.addIssue({
            code: "custom",
            path: [field],
            message: `${field} is only valid for agent steps`,
          });
        }
      }
    }
    if (
      step.source === "agent" &&
      step.llm_call_count === 0 &&
      (step.metrics !== undefined || step.reasoning_content !== undefined)
    ) {
      ctx.addIssue({
        code: "custom",
        path: ["llm_call_count"],
        message:
          "metrics and reasoning_content are not valid when llm_call_count is zero",
      });
    }
  });

export type AtifStep = z.infer<typeof AtifStepSchema>;

export const ATIF_SCHEMA_VERSION = "ATIF-v1.8" as const;

export const ATIF_SCHEMA_VERSIONS = [
  "ATIF-v1.0",
  "ATIF-v1.1",
  "ATIF-v1.2",
  "ATIF-v1.3",
  "ATIF-v1.4",
  "ATIF-v1.5",
  "ATIF-v1.6",
  "ATIF-v1.7",
  "ATIF-v1.8",
] as const;

export const AtifSchemaVersionSchema = z.enum(ATIF_SCHEMA_VERSIONS);

export type AtifSchemaVersion = z.infer<typeof AtifSchemaVersionSchema>;

export const AtifTrajectorySchema = z
  .strictObject({
    schema_version: AtifSchemaVersionSchema,
    session_id: z.string().optional(),
    trajectory_id: z.string().optional(),
    agent: AtifAgentSchema,
    steps: z.array(AtifStepSchema).min(1),
    notes: z.string().optional(),
    final_metrics: AtifFinalMetricsSchema.optional(),
    continued_trajectory_ref: z.string().optional(),
    extra: z.record(z.string(), z.unknown()).optional(),
    get subagent_trajectories() {
      return z.array(AtifTrajectorySchema).optional();
    },
  })
  .superRefine((trajectory, ctx) => {
    trajectory.steps.forEach((step, index) => {
      if (step.step_id !== index + 1) {
        ctx.addIssue({
          code: "custom",
          path: ["steps", index, "step_id"],
          message: "step_id values must be sequential starting at 1",
        });
      }
      const toolCallIds = new Set(
        (step.tool_calls ?? []).map((toolCall) => toolCall.tool_call_id)
      );
      for (const [resultIndex, result] of (
        step.observation?.results ?? []
      ).entries()) {
        if (
          result.source_call_id !== undefined &&
          !toolCallIds.has(result.source_call_id)
        ) {
          ctx.addIssue({
            code: "custom",
            path: ["steps", index, "observation", "results", resultIndex],
            message: "source_call_id must reference a tool call in this step",
          });
        }
      }
    });

    const subagents = trajectory.subagent_trajectories ?? [];
    const trajectoryIds = new Set<string>();
    subagents.forEach((subagent, index) => {
      if (subagent.trajectory_id === undefined) {
        ctx.addIssue({
          code: "custom",
          path: ["subagent_trajectories", index, "trajectory_id"],
          message: "embedded subagent trajectories require trajectory_id",
        });
        return;
      }
      if (trajectoryIds.has(subagent.trajectory_id)) {
        ctx.addIssue({
          code: "custom",
          path: ["subagent_trajectories", index, "trajectory_id"],
          message: "subagent trajectory_id values must be unique",
        });
        return;
      }
      trajectoryIds.add(subagent.trajectory_id);
    });
  });

export type AtifTrajectory = z.infer<typeof AtifTrajectorySchema>;
