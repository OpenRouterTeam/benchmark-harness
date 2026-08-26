import type { ChatFunctionToolFunction } from "@openrouter/sdk/models/chatfunctiontool";
import type { ChatToolCall } from "@openrouter/sdk/models/chattoolcall";
import { TaggedError } from "effect/Data";

import type { ValueOf } from "../internal/guards";
import { z } from "../internal/zod";
import type { ModelErrorIdentifiers } from "../providers/request-identifiers";
import type { ImageDetail } from "./constants";
import { IMAGE_DETAIL_VALUES } from "./constants";
import { ReasoningDetailsSchema } from "./reasoning-details";

export const MessageRole = {
  System: "system",
  User: "user",
  Assistant: "assistant",
  Tool: "tool",
} as const;

export type MessageRole = ValueOf<typeof MessageRole>;

export const MESSAGE_ROLE_VALUES = [
  MessageRole.System,
  MessageRole.User,
  MessageRole.Assistant,
  MessageRole.Tool,
] as const;

export type ToolCall = ChatToolCall;

export type ToolDefinition = ChatFunctionToolFunction;

export const ChatToolCallSchema = z.object({
  id: z.string(),
  type: z.literal("function"),
  function: z.object({ name: z.string(), arguments: z.string() }),
});

export const CitationSchema = z.object({
  url: z.string(),
  title: z.string(),
  startIndex: z.number(),
  endIndex: z.number(),
});

export const TextContentPartSchema = z.object({
  type: z.literal("text"),
  text: z.string(),
});

export const ImageContentPartSchema = z.object({
  type: z.literal("image_url"),
  imageUrl: z.object({
    url: z.string(),
    detail: z.enum(IMAGE_DETAIL_VALUES).optional(),
  }),
});

export const VideoContentPartSchema = z.object({
  type: z.literal("video_url"),
  videoUrl: z.object({
    url: z.string(),
  }),
});

export const ContentPartSchema = z.discriminatedUnion("type", [
  TextContentPartSchema,
  ImageContentPartSchema,
  VideoContentPartSchema,
]);

export const ModelMessageSchema = z
  .object({
    role: z.enum(MESSAGE_ROLE_VALUES),
    content: z.string(),
    contentParts: z.array(ContentPartSchema).readonly().optional(),
    toolCalls: z.array(ChatToolCallSchema).readonly().optional(),
    toolCallId: z.string().optional(),
    reasoning: z.string().optional(),
    reasoningDetails: ReasoningDetailsSchema.optional(),
    responseItems: z
      .array(z.record(z.string(), z.unknown()))
      .readonly()
      .optional(),
    citations: z.array(CitationSchema).readonly().optional(),
    model: z.string().optional(),
  })
  .readonly();

export interface Citation {
  readonly url: string;
  readonly title: string;
  readonly startIndex: number;
  readonly endIndex: number;
}

export interface TextContentPart {
  readonly type: "text";
  readonly text: string;
}

export interface ImageContentPart {
  readonly type: "image_url";
  readonly imageUrl: {
    readonly url: string;
    readonly detail?: ImageDetail;
  };
}

export interface VideoContentPart {
  readonly type: "video_url";
  readonly videoUrl: {
    readonly url: string;
  };
}

export type ContentPart = TextContentPart | ImageContentPart | VideoContentPart;

export type ModelMessage = z.infer<typeof ModelMessageSchema>;

export interface Sample {
  readonly id: string;
  readonly input: string;
  readonly target: Target;
  readonly contentParts?: readonly ContentPart[];
  readonly metadata?: Readonly<Record<string, unknown>>;
}

export interface Target {
  readonly text: string;
}

export interface UsageTotals {
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly totalTokens: number;
  readonly reasoningTokens: number;
  readonly totalCost: number;
  readonly generationTimeMs: number;
}

export interface ServerToolUseCounts {
  readonly webSearchRequests?: number;
  readonly toolCallsRequested?: number;
  readonly toolCallsExecuted?: number;
}

export type ModelUsage = Partial<Omit<UsageTotals, "generationTimeMs">> & {
  readonly serverToolUse?: ServerToolUseCounts;
};

export type ResponseItem = Readonly<Record<string, unknown>>;

export interface ModelOutput {
  readonly completion: string;
  readonly message: ModelMessage;
  readonly usage?: ModelUsage;
  readonly generationTimeMs?: number;
  readonly rawResponse?: Readonly<Record<string, unknown>>;
}

export interface TaskState {
  readonly sample: Sample;
  readonly messages: readonly ModelMessage[];
  readonly responseItems?: readonly ResponseItem[];
  readonly requestBody?: Readonly<Record<string, unknown>>;
  readonly output?: ModelOutput;
  readonly completed: boolean;
  readonly epoch?: number;
}

export function initialTaskState(sample: Sample, epoch?: number): TaskState {
  return {
    sample,
    messages: [
      {
        role: MessageRole.User,
        content: sample.input,
        ...(sample.contentParts !== undefined && {
          contentParts: sample.contentParts,
        }),
      },
    ],
    output: undefined,
    completed: false,
    ...(epoch !== undefined && { epoch }),
  };
}

export const ScoreValue = {
  Correct: "C",
  Incorrect: "I",
  Skipped: "S",
} as const;

export type ScoreValue = ValueOf<typeof ScoreValue>;

export type ScorerTrajectory =
  | {
      readonly kind: "verifier_log";
      readonly log: string;
    }
  | {
      readonly kind: "judge_runs";
      readonly runs: readonly unknown[];
    };

export interface Score {
  readonly value: ScoreValue;
  readonly answer: string | null;
  readonly explanation: string;
  readonly trajectory?: ScorerTrajectory;
}

export function scoreToNumber(value: ScoreValue): number {
  switch (value) {
    case ScoreValue.Correct: {
      return 1;
    }
    case ScoreValue.Incorrect:
    case ScoreValue.Skipped: {
      return 0;
    }
    default: {
      value satisfies never;
      return 0;
    }
  }
}

export class ModelError extends TaggedError("ModelError")<
  {
    readonly status?: number;
    readonly message: string;
    readonly retryAfterMs?: number;
  } & ModelErrorIdentifiers
> {}

export function isRetryableModelError(error: ModelError): boolean {
  return (
    error.status === 429 || (error.status !== undefined && error.status >= 500)
  );
}

const SYSTEMIC_STATUS_CODES = new Set([401, 403, 404]);

export function isSystemicModelError(error: ModelError): boolean {
  if (error.status === undefined) {
    return true;
  }
  return SYSTEMIC_STATUS_CODES.has(error.status);
}

export class SolverError extends TaggedError("SolverError")<{
  readonly message: string;
}> {}

export class DatasetError extends TaggedError("DatasetError")<{
  readonly message: string;
}> {}
