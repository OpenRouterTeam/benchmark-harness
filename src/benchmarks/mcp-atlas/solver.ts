import { gen } from "effect/Effect";
import * as Result from "effect/Result";

import type { TaskState, ToolDefinition } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import type { FixedTemperatureInferenceOverride } from "../benchmark-config";
import type { McpSandbox, McpTool } from "../mcp-sandbox/sandbox";
import { toToolDefinition } from "../mcp-sandbox/sandbox";
import { McpRunStatsSchema, runMcpToolLoop } from "../mcp-sandbox/tool-loop";
import { MCP_ATLAS_TASK_METADATA_KEY, McpAtlasTaskSchema } from "./dataset";
import type { McpAtlasToolScope } from "./schema";

export const MCP_ATLAS_TEMPERATURE = 0;

export const MCP_ATLAS_RUN_METADATA_KEY = "mcp_atlas_run";

export const McpAtlasRunMetadataSchema = McpRunStatsSchema.extend({
  missingTools: z.array(z.string()).readonly(),
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

export function mcpAtlasSolver({
  model,
  sandbox,
  tools,
  opts,
}: {
  readonly model: ModelService;
  readonly sandbox: McpSandbox;
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
      const loop = yield* runMcpToolLoop({
        model,
        sandbox,
        tools: selected.definitions,
        messages: state.messages,
        genConfig,
        opts: {
          maxTurns: opts.maxTurns,
          maxToolCalls: opts.maxToolCalls,
          ...definedValues({ toolOutputCap: opts.toolOutputCap }),
        },
      });
      const run: McpAtlasRunMetadata = {
        ...loop.stats,
        missingTools: selected.missingTools,
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
