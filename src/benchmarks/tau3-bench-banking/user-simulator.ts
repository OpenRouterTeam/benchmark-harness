import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { map, mapError } from "effect/Effect";

import type { ModelMessage, ToolDefinition } from "../../harness/core";
import { MessageRole } from "../../harness/core";
import { definedValues } from "../../internal/guards";
import {
  messagesToResponses,
  toolDefinitionToResponses,
} from "../../providers/messages-to-responses";
import type { ResponsesModelService } from "../../providers/responses-model";
import type { UserModelConfig } from "./types";
import {
  USER_SIM_GUIDELINES,
  USER_SIM_GUIDELINES_TOOLS,
} from "./user-sim-guidelines";

class UserSimError extends TaggedError("UserSimError")<{
  readonly message: string;
}> {}

type SimError = UserSimError;

export type SimulatorTurn = TextTurn | ToolCallsTurn;

interface TextTurn {
  readonly kind: "text";
  readonly content: string;
}

interface ToolCallsTurn {
  readonly kind: "toolCalls";
  readonly calls: readonly {
    readonly id: string;
    readonly name: string;
    readonly arguments: string;
  }[];
}

function buildUserSystemPrompt(
  scenarioInstructions: string,
  useTools: boolean
): string {
  const guidelines = useTools ? USER_SIM_GUIDELINES_TOOLS : USER_SIM_GUIDELINES;
  return `${guidelines}\n\n<scenario>\n${scenarioInstructions}\n</scenario>`;
}

export class UserSimulator {
  private readonly messages: ModelMessage[] = [];
  private readonly config: UserModelConfig;
  private readonly model: ResponsesModelService;
  private availableTools: readonly ToolDefinition[] = [];

  constructor(model: ResponsesModelService, config: UserModelConfig) {
    this.model = model;
    this.config = config;
  }

  reset(scenarioInstructions: string, firstAgentMessage: string): void {
    const useTools = this.availableTools.length > 0;
    this.messages.length = 0;
    this.messages.push(
      {
        role: MessageRole.System,
        content: buildUserSystemPrompt(scenarioInstructions, useTools),
      },
      { role: MessageRole.User, content: firstAgentMessage }
    );
  }

  generateInitial(): Effect<SimulatorTurn, SimError> {
    return this.callModel(this.config.model);
  }

  step(agentMessage: string): Effect<SimulatorTurn, SimError> {
    this.messages.push({ role: MessageRole.User, content: agentMessage });
    return this.callModel(this.config.model);
  }

  continueAfterTools(): Effect<SimulatorTurn, SimError> {
    return this.callModel(this.config.model);
  }

  setAvailableTools(toolDefs: readonly ToolDefinition[]): void {
    this.availableTools = toolDefs;
  }

  addToolResult(toolCallId: string, content: string): void {
    this.messages.push({
      role: MessageRole.Tool,
      content,
      toolCallId,
    });
  }

  private callModel(model: string): Effect<SimulatorTurn, SimError> {
    return this.callModelOnce(model).pipe(
      map((turn) => {
        const assistantMessage: ModelMessage = definedValues({
          role: MessageRole.Assistant,
          content: turn.text,
          responseItems:
            turn.outputItems.length > 0 ? turn.outputItems : undefined,
        });
        this.messages.push(assistantMessage);
        if (turn.functionCalls.length > 0) {
          return {
            kind: "toolCalls",
            calls: turn.functionCalls.map((call) => ({
              id: call.callId,
              name: call.name,
              arguments: call.arguments,
            })),
          };
        }
        return { kind: "text", content: turn.text };
      })
    );
  }

  private callModelOnce(model: string) {
    return this.model
      .generate(messagesToResponses(this.messages), {
        model,
        temperature: 0,
        reasoningEffort: this.config.userReasoningEffort,
        ...definedValues({
          tools:
            this.availableTools.length > 0
              ? this.availableTools.map(toolDefinitionToResponses)
              : undefined,
        }),
      })
      .pipe(mapError((error) => new UserSimError({ message: error.message })));
  }
}
