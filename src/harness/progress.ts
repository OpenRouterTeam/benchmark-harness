import { Tag } from "effect/Context";
import type { Effect } from "effect/Effect";
import { sync } from "effect/Effect";

import { z } from "../internal/zod";

export type SampleProgressEvent =
  | SampleStartEvent
  | SampleEndEvent
  | TurnEvent
  | ToolCallEvent
  | SubmitEvent;

export interface SampleStartEvent {
  readonly type: "sample-start";
  readonly sampleIndex: number;
  readonly sampleId: string;
  readonly epoch: number;
}

export interface SampleEndEvent {
  readonly type: "sample-end";
  readonly sampleId: string;
  readonly epoch: number;
}

export interface TurnEvent {
  readonly type: "turn";
  readonly sampleId: string;
  readonly epoch: number;
  readonly step: number;
  readonly toolCallIndex: number;
}

export interface ToolCallEvent {
  readonly type: "tool-call";
  readonly sampleId: string;
  readonly epoch: number;
  readonly step: number;
  readonly toolCallIndex: number;
  readonly command: string;
}

export interface SubmitEvent {
  readonly type: "submit";
  readonly sampleId: string;
  readonly epoch: number;
  readonly step: number;
  readonly toolCallIndex: number;
}

export interface ProgressReporterService {
  readonly onSampleStart: (event: SampleStartEvent) => Effect<void>;
  readonly onSampleEnd: (event: SampleEndEvent) => Effect<void>;
  readonly onSampleComplete: (count: number) => Effect<void>;
  readonly onAgentStep: (
    event: AgentStepEvent,
    sampleId: string,
    epoch: number
  ) => Effect<void>;
}

export class ProgressReporter extends Tag(
  "@openrouter/bench-harness/progress-reporter"
)<ProgressReporter, ProgressReporterService>() {}

export const NOOP_PROGRESS_REPORTER: ProgressReporterService = {
  onSampleStart: () => sync(() => {}),
  onSampleEnd: () => sync(() => {}),
  onSampleComplete: () => sync(() => {}),
  onAgentStep: () => sync(() => {}),
};

export function makeProgressReporter(callbacks: {
  readonly onSampleStart?: (event: SampleStartEvent) => void;
  readonly onSampleEnd?: (event: SampleEndEvent) => void;
  readonly onSampleComplete?: (count: number) => void;
  readonly onAgentStep?: (
    event: AgentStepEvent,
    sampleId: string,
    epoch: number
  ) => void;
}): ProgressReporterService {
  return {
    onSampleStart: (e) => sync(() => callbacks.onSampleStart?.(e)),
    onSampleEnd: (e) => sync(() => callbacks.onSampleEnd?.(e)),
    onSampleComplete: (n) => sync(() => callbacks.onSampleComplete?.(n)),
    onAgentStep: (e, id, ep) => sync(() => callbacks.onAgentStep?.(e, id, ep)),
  };
}

export type AgentStepEvent =
  | AgentTurnEvent
  | AgentToolCallEvent
  | AgentSubmitEvent;

export interface AgentTurnEvent {
  readonly type: "turn";
  readonly step: number;
  readonly toolCallIndex: number;
}

export interface AgentToolCallEvent {
  readonly type: "tool-call";
  readonly step: number;
  readonly toolCallIndex: number;
  readonly command: string;
}

export interface AgentSubmitEvent {
  readonly type: "submit";
  readonly step: number;
  readonly toolCallIndex: number;
}

export const CheckpointDataSchema = z.object({
  sandboxId: z.string(),
  input: z.array(z.record(z.string(), z.unknown())),
  step: z.number(),
  usage: z
    .object({
      inputTokens: z.number(),
      outputTokens: z.number(),
      totalTokens: z.number(),
      reasoningTokens: z.number(),
      totalCost: z.number(),
      webSearchRequests: z.number().optional(),
      toolCallsRequested: z.number().optional(),
      toolCallsExecuted: z.number().optional(),
      seenServerToolUse: z.boolean().optional(),
    })
    .optional(),
  generationTimeMs: z.number().optional(),
  toolCallIndex: z.number().optional(),
});

export type CheckpointData = z.infer<typeof CheckpointDataSchema>;

export interface CheckpointStoreService {
  readonly read: (key: string) => Promise<CheckpointData | null>;
  readonly write: (key: string, data: CheckpointData) => Promise<void>;
  readonly remove: (key: string) => Promise<void>;
}

export class CheckpointStore extends Tag(
  "@openrouter/bench-harness/checkpoint-store"
)<CheckpointStore, CheckpointStoreService>() {}

export const NOOP_CHECKPOINT_STORE: CheckpointStoreService = {
  read: async () => null,
  write: async () => {},
  remove: async () => {},
};
