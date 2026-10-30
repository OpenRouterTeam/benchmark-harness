import type { Effect } from "effect/Effect";
import { gen, succeed } from "effect/Effect";
import * as Result from "effect/Result";

import type {
  ModelMessage,
  ModelUsage,
  TaskState,
  ToolCall,
  ToolDefinition,
} from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import type { ValueOf } from "../../internal/guards";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import { withCallCacheSalt } from "../../runtime/response-cache";
import type { FixedTemperatureInferenceOverride } from "../benchmark-config";
import { MCP_ATLAS_TASK_METADATA_KEY, McpAtlasTaskSchema } from "./dataset";
import type { McpAtlasSandbox, McpTool, McpToolResult } from "./sandbox";
import { toToolDefinition } from "./sandbox";
import type { McpAtlasToolScope } from "./schema";

export const MCP_ATLAS_TEMPERATURE = 0;

export const MCP_ATLAS_RUN_METADATA_KEY = "mcp_atlas_run";

export const TerminationReason = {
  Completed: "completed",
  MaxTurns: "max_turns",
  MaxToolCalls: "max_tool_calls",
} as const;

export type TerminationReason = ValueOf<typeof TerminationReason>;

export const McpAtlasRunMetadataSchema = z.object({
  terminationReason: z.enum([
    TerminationReason.Completed,
    TerminationReason.MaxTurns,
    TerminationReason.MaxToolCalls,
  ]),
  turnCount: z.number(),
  toolCallCount: z.number(),
  toolErrorCount: z.number(),
  toolDefinitionCount: z.number(),
  missingTools: z.array(z.string()).readonly(),
  toolCallNames: z.array(z.string()).readonly(),
});

export type McpAtlasRunMetadata = z.infer<typeof McpAtlasRunMetadataSchema>;

export interface McpAtlasSolverOptions {
  readonly toolScope: McpAtlasToolScope;
  readonly maxTurns: number;
  readonly maxToolCalls: number;
  readonly toolOutputCap?: number;
  readonly endpointId?: string;
  readonly inference: FixedTemperatureInferenceOverride;
}

export interface SelectedTools {
  readonly definitions: readonly ToolDefinition[];
  readonly missingTools: readonly string[];
}

const TOOL_BUDGET_EXHAUSTED =
  "Error: the tool call budget for this task is exhausted; answer with what you have.";

export function selectTools(
  tools: readonly McpTool[],
  enabledTools: readonly string[],
  scope: McpAtlasToolScope
): SelectedTools {
  if (scope === "all") {
    return { definitions: tools.map(toToolDefinition), missingTools: [] };
  }
  const available = new Set(tools.map((tool) => tool.name));
  const enabled = new Set(enabledTools);
  return {
    definitions: tools
      .filter((tool) => enabled.has(tool.name))
      .map(toToolDefinition),
    missingTools: enabledTools.filter((name) => !available.has(name)),
  };
}

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
  sandbox: McpAtlasSandbox,
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

export function mcpAtlasSolver({
  model,
  sandbox,
  tools,
  opts,
}: {
  readonly model: ModelService;
  readonly sandbox: McpAtlasSandbox;
  readonly tools: readonly McpTool[];
  readonly opts: McpAtlasSolverOptions;
}): SolverService {
  return (state: TaskState) =>
    gen(function* () {
      const taskParsed = parseSchema(
        McpAtlasTaskSchema,
        state.sample.metadata?.[MCP_ATLAS_TASK_METADATA_KEY]
      );
      if (Result.isFailure(taskParsed)) {
        return yield* new SolverError({
          message: `Invalid MCP-Atlas task in metadata: ${taskParsed.failure.message}`,
        });
      }
      const selected = selectTools(
        tools,
        taskParsed.success.enabledTools,
        opts.toolScope
      );
      const genConfig: GenerateConfig = {
        temperature: MCP_ATLAS_TEMPERATURE,
        tools: selected.definitions,
        ...definedValues(opts.inference),
        ...definedValues({ endpointId: opts.endpointId }),
      };
      const messages: ModelMessage[] = [...state.messages];
      const toolCallNames: string[] = [];
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
      const completion = lastAssistantContent(messages);
      const finalUsage: ModelUsage = usage;
      const run: McpAtlasRunMetadata = {
        terminationReason,
        turnCount,
        toolCallCount,
        toolErrorCount,
        toolDefinitionCount: selected.definitions.length,
        missingTools: selected.missingTools,
        toolCallNames,
      };
      return {
        ...state,
        sample: {
          ...state.sample,
          metadata: {
            ...state.sample.metadata,
            [MCP_ATLAS_RUN_METADATA_KEY]: run,
          },
        },
        messages,
        output: {
          completion,
          message: { role: MessageRole.Assistant, content: completion },
          usage: finalUsage,
          generationTimeMs,
        },
        completed: true,
      };
    });
}
