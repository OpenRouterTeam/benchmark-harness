import { gen } from "effect/Effect";
import * as Result from "effect/Result";

import type { ModelMessage, TaskState } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import type { FixedTemperatureInferenceOverride } from "../benchmark-config";
import type { McpSandbox, McpTool } from "../mcp-sandbox/sandbox";
import { toToolDefinition } from "../mcp-sandbox/sandbox";
import { McpRunStatsSchema, runMcpToolLoop } from "../mcp-sandbox/tool-loop";
import {
  LIVE_MCP_BENCH_TASK_METADATA_KEY,
  LiveMcpBenchTaskSchema,
} from "./dataset";
import type { LiveMcpBenchToolScope } from "./schema";
import { LIVE_MCP_BENCH_SYSTEM_PROMPT } from "./schema";

export const LIVE_MCP_BENCH_TEMPERATURE = 0;

export const LIVE_MCP_BENCH_RUN_METADATA_KEY = "live_mcp_bench_run";

export const LiveMcpBenchRunMetadataSchema = McpRunStatsSchema.extend({
  calledToolDescriptions: z.record(z.string(), z.string()).readonly(),
});

export type LiveMcpBenchRunMetadata = z.infer<
  typeof LiveMcpBenchRunMetadataSchema
>;

export interface LiveMcpBenchSolverOptions {
  readonly toolScope: LiveMcpBenchToolScope;
  readonly maxTurns: number;
  readonly maxToolCalls: number;
  readonly toolOutputCap: number;
  readonly endpointId?: string;
  readonly inference: FixedTemperatureInferenceOverride;
}

const SERVER_SEPARATOR = "__";

export function matchesAnnotatedTool(
  toolName: string,
  annotated: string
): boolean {
  return (
    toolName === annotated ||
    toolName.endsWith(`${SERVER_SEPARATOR}${annotated}`)
  );
}

export function selectTools(
  tools: readonly McpTool[],
  annotatedTools: readonly string[],
  scope: LiveMcpBenchToolScope
): readonly McpTool[] {
  if (scope === "all") {
    return tools;
  }
  return tools.filter((tool) =>
    annotatedTools.some((annotated) =>
      matchesAnnotatedTool(tool.name, annotated)
    )
  );
}

export function withSystemPrompt(
  messages: readonly ModelMessage[]
): readonly ModelMessage[] {
  if (messages.some((message) => message.role === MessageRole.System)) {
    return messages;
  }
  return [
    { role: MessageRole.System, content: LIVE_MCP_BENCH_SYSTEM_PROMPT },
    ...messages,
  ];
}

export function calledToolDescriptions(
  tools: readonly McpTool[],
  calledNames: readonly string[]
): Readonly<Record<string, string>> {
  const byName = new Map(tools.map((tool) => [tool.name, tool]));
  return Object.fromEntries(
    [...new Set(calledNames)].map((name) => [
      name,
      byName.get(name)?.description ?? "(no description)",
    ])
  );
}

export function liveMcpBenchSolver({
  model,
  sandbox,
  tools,
  opts,
}: {
  readonly model: ModelService;
  readonly sandbox: McpSandbox;
  readonly tools: readonly McpTool[];
  readonly opts: LiveMcpBenchSolverOptions;
}): SolverService {
  return (state: TaskState) =>
    gen(function* () {
      const taskParsed = parseSchema(
        LiveMcpBenchTaskSchema,
        state.sample.metadata?.[LIVE_MCP_BENCH_TASK_METADATA_KEY]
      );
      if (Result.isFailure(taskParsed)) {
        return yield* new SolverError({
          message: `Invalid LiveMCPBench task in metadata: ${taskParsed.failure.message}`,
        });
      }
      const selected = selectTools(
        tools,
        taskParsed.success.annotatedTools,
        opts.toolScope
      );
      const definitions = selected.map(toToolDefinition);
      const genConfig: GenerateConfig = {
        temperature: LIVE_MCP_BENCH_TEMPERATURE,
        tools: definitions,
        ...definedValues(opts.inference),
        ...definedValues({ endpointId: opts.endpointId }),
      };
      const loop = yield* runMcpToolLoop({
        model,
        sandbox,
        tools: definitions,
        messages: withSystemPrompt(state.messages),
        genConfig,
        opts: {
          maxTurns: opts.maxTurns,
          maxToolCalls: opts.maxToolCalls,
          toolOutputCap: opts.toolOutputCap,
        },
      });
      const run: LiveMcpBenchRunMetadata = {
        ...loop.stats,
        calledToolDescriptions: calledToolDescriptions(
          selected,
          loop.stats.toolCallNames
        ),
      };
      return {
        ...state,
        sample: {
          ...state.sample,
          metadata: {
            ...state.sample.metadata,
            [LIVE_MCP_BENCH_RUN_METADATA_KEY]: run,
          },
        },
        messages: loop.messages,
        output: {
          completion: loop.completion,
          message: { role: MessageRole.Assistant, content: loop.completion },
          usage: loop.usage,
          generationTimeMs: loop.generationTimeMs,
        },
        completed: true,
      };
    });
}
