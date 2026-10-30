import type { Effect } from "effect/Effect";
import { gen, succeed } from "effect/Effect";
import * as Result from "effect/Result";

import type {
  ModelError,
  ModelMessage,
  ModelUsage,
  ToolCall,
  ToolDefinition,
} from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type { ValueOf } from "../../internal/guards";
import { z } from "../../internal/zod";
import { withCallCacheSalt } from "../../runtime/response-cache";
import type { McpSandbox, McpToolResult } from "./sandbox";

export const TerminationReason = {
  Completed: "completed",
  MaxTurns: "max_turns",
  MaxToolCalls: "max_tool_calls",
} as const;

export type TerminationReason = ValueOf<typeof TerminationReason>;

export const McpToolCallRecordSchema = z.object({
  name: z.string(),
  arguments: z.string(),
  output: z.string(),
  isError: z.boolean(),
});

export type McpToolCallRecord = z.infer<typeof McpToolCallRecordSchema>;

export const DEFAULT_TRANSCRIPT_OUTPUT_CAP = 8_000;

export const McpRunStatsSchema = z.object({
  terminationReason: z.enum([
    TerminationReason.Completed,
    TerminationReason.MaxTurns,
    TerminationReason.MaxToolCalls,
  ]),
  turnCount: z.number(),
  toolCallCount: z.number(),
  toolErrorCount: z.number(),
  toolDefinitionCount: z.number(),
  toolCallNames: z.array(z.string()).readonly(),
  transcript: z.array(McpToolCallRecordSchema).readonly().default([]),
});

export type McpRunStats = z.infer<typeof McpRunStatsSchema>;

export interface McpToolLoopOptions {
  readonly maxTurns: number;
  readonly maxToolCalls: number;
  readonly toolOutputCap?: number;
  readonly transcriptOutputCap?: number;
}

export interface McpToolLoopInput {
  readonly model: ModelService;
  readonly sandbox: McpSandbox;
  readonly tools: readonly ToolDefinition[];
  readonly messages: readonly ModelMessage[];
  readonly genConfig: GenerateConfig;
  readonly opts: McpToolLoopOptions;
}

export interface McpToolLoopOutput {
  readonly messages: readonly ModelMessage[];
  readonly completion: string;
  readonly usage: ModelUsage;
  readonly generationTimeMs: number;
  readonly stats: McpRunStats;
}

const TOOL_BUDGET_EXHAUSTED =
  "Error: the tool call budget for this task is exhausted; answer with what you have.";

export function capToolOutput(text: string, cap: number | undefined): string {
  if (cap === undefined || text.length <= cap) {
    return text;
  }
  return `${text.slice(0, cap)}\n\n[Tool output truncated to ${cap} chars. Original was ${text.length} chars.]`;
}

export function parseToolArguments(
  raw: string
): Result.Result<Readonly<Record<string, unknown>>, string> {
  const trimmed = raw.trim();
  if (trimmed === "") {
    return Result.succeed({});
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch (error) {
    return Result.fail(
      `tool arguments are not valid JSON: ${error instanceof Error ? error.message : String(error)}`
    );
  }
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    return Result.fail("tool arguments must be a JSON object");
  }
  return Result.succeed(parsed as Readonly<Record<string, unknown>>);
}

function executeToolCall(
  sandbox: McpSandbox,
  call: ToolCall
): Effect<McpToolResult, SolverError> {
  const args = parseToolArguments(call.function.arguments);
  if (Result.isFailure(args)) {
    return succeed({ text: `Error: ${args.failure}`, isError: true });
  }
  return sandbox.callTool(call.function.name, args.success);
}

function lastAssistantContent(messages: readonly ModelMessage[]): string {
  const assistant = messages.findLast(
    (message) => message.role === MessageRole.Assistant
  );
  return assistant?.content ?? "";
}

export function runMcpToolLoop(
  input: McpToolLoopInput
): Effect<McpToolLoopOutput, SolverError | ModelError> {
  const { model, sandbox, genConfig, opts } = input;
  return gen(function* () {
    const messages: ModelMessage[] = [...input.messages];
    const toolCallNames: string[] = [];
    const transcript: McpToolCallRecord[] = [];
    const usage = {
      inputTokens: 0,
      outputTokens: 0,
      totalTokens: 0,
      reasoningTokens: 0,
      totalCost: 0,
    };
    let generationTimeMs = 0;
    let turnCount = 0;
    let toolCallCount = 0;
    let toolErrorCount = 0;
    let terminationReason: TerminationReason = TerminationReason.MaxTurns;

    while (turnCount < opts.maxTurns) {
      const output = yield* withCallCacheSalt(
        `turn-${turnCount}`,
        model.generate(messages, genConfig)
      );
      turnCount += 1;
      generationTimeMs += output.generationTimeMs ?? 0;
      usage.inputTokens += output.usage?.inputTokens ?? 0;
      usage.outputTokens += output.usage?.outputTokens ?? 0;
      usage.totalTokens += output.usage?.totalTokens ?? 0;
      usage.reasoningTokens += output.usage?.reasoningTokens ?? 0;
      usage.totalCost += output.usage?.totalCost ?? 0;
      messages.push(output.message);

      const toolCalls = output.message.toolCalls ?? [];
      if (toolCalls.length === 0) {
        terminationReason = TerminationReason.Completed;
        break;
      }

      let budgetExhausted = false;
      for (const call of toolCalls) {
        if (toolCallCount >= opts.maxToolCalls) {
          budgetExhausted = true;
          messages.push({
            role: MessageRole.Tool,
            content: TOOL_BUDGET_EXHAUSTED,
            toolCallId: call.id,
          });
          continue;
        }
        toolCallCount += 1;
        toolCallNames.push(call.function.name);
        const result = yield* executeToolCall(sandbox, call);
        if (result.isError) {
          toolErrorCount += 1;
        }
        transcript.push({
          name: call.function.name,
          arguments: call.function.arguments,
          output: capToolOutput(
            result.text,
            opts.transcriptOutputCap ?? DEFAULT_TRANSCRIPT_OUTPUT_CAP
          ),
          isError: result.isError,
        });
        messages.push({
          role: MessageRole.Tool,
          content: capToolOutput(result.text, opts.toolOutputCap),
          toolCallId: call.id,
        });
      }
      if (budgetExhausted) {
        terminationReason = TerminationReason.MaxToolCalls;
        break;
      }
    }

    return {
      messages,
      completion: lastAssistantContent(messages),
      usage,
      generationTimeMs,
      stats: {
        terminationReason,
        turnCount,
        toolCallCount,
        toolErrorCount,
        toolDefinitionCount: input.tools.length,
        toolCallNames,
        transcript,
      },
    };
  });
}
