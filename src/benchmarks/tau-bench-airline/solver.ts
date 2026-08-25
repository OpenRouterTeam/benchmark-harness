import { HttpClient } from "@effect/platform";
import type { Semaphore } from "effect/Effect";
import { gen, mapError, provideService } from "effect/Effect";

import type { ChatMessage, ModelUsage, ToolCall } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import { Either } from "../../internal/either";
import { definedValues, isRecord } from "../../internal/guards";
import type { ResponsesModelService } from "../../providers/responses-model";
import {
  buildAgentSystemPrompt,
  DEFAULT_FIRST_AGENT_MESSAGE,
} from "./agent-prompts";
import { ensureAirlineData, loadAirlineData } from "./environment";
import { AIRLINE_POLICY } from "./policy";
import { AIRLINE_TOOL_DEFINITIONS } from "./tools/definitions";
import { invokeTool } from "./tools/handlers";
import type { AirlineData, SolverOpts } from "./types";
import { UserSimulator } from "./user-simulator";

const STOP_TOKENS = ["###STOP###", "###TRANSFER###", "###OUT-OF-SCOPE###"];

const MAX_STEPS = 200;

const AIRLINE_TEMPERATURE = 0;

export const TerminationReason = {
  UserStop: "USER_STOP",
  MaxSteps: "MAX_STEPS",
} as const;

const Role = {
  User: "user",
  Agent: "agent",
  Env: "env",
} as const;

type Role = (typeof Role)[keyof typeof Role];

export function airlineSolver({
  model,
  userModel,
  client,
  dataFetchLock,
  opts,
}: {
  readonly model: ModelService;
  readonly userModel: ResponsesModelService;
  readonly client: HttpClient.HttpClient;
  readonly dataFetchLock: Semaphore;
  readonly opts?: SolverOpts;
}): SolverService {
  return (state) =>
    gen(function* () {
      const userModelConfig = opts?.userModelConfig;
      if (!userModelConfig) {
        return yield* new SolverError({
          message:
            "airlineSolver requires userModelConfig (apiKey + model for user simulator)",
        });
      }
      yield* ensureAirlineData(dataFetchLock).pipe(
        provideService(HttpClient.HttpClient, client),
        mapError(
          (e) =>
            new SolverError({
              message: `Failed to load airline data from HF: ${String(e)}`,
            })
        )
      );
      const task = state.sample.metadata?.["task"];
      const data: AirlineData = loadAirlineData();
      const userSim = new UserSimulator(userModel, userModelConfig);
      userSim.reset(state.sample.input, DEFAULT_FIRST_AGENT_MESSAGE);
      const messages: ChatMessage[] = [
        {
          role: MessageRole.System,
          content: buildAgentSystemPrompt(AIRLINE_POLICY),
        },
        { role: MessageRole.Assistant, content: DEFAULT_FIRST_AGENT_MESSAGE },
      ];
      const respondMessages: string[] = [];
      const genConfig: GenerateConfig = {
        temperature: AIRLINE_TEMPERATURE,
        tools: AIRLINE_TOOL_DEFINITIONS,
        ...definedValues(opts?.inference ?? {}),
        ...(opts?.endpointId !== undefined && { endpointId: opts.endpointId }),
      };
      let totalGenerationTimeMs = 0;
      const accUsage = {
        inputTokens: 0,
        outputTokens: 0,
        totalTokens: 0,
        reasoningTokens: 0,
        totalCost: 0,
      };
      let role: Role = Role.User;
      let stepCount = 0;
      let isFirstUserTurn = true;
      let terminationReason: string = TerminationReason.MaxSteps;
      let done = false;
      while (!done) {
        if (stepCount >= MAX_STEPS) {
          terminationReason = TerminationReason.MaxSteps;
          break;
        }
        switch (role) {
          case Role.User: {
            const userResponse = yield* (
              isFirstUserTurn
                ? userSim.generateInitial()
                : userSim.step(lastAssistantText(messages))
            ).pipe(
              provideService(HttpClient.HttpClient, client),
              mapError(
                (e) =>
                  new SolverError({
                    message: `User simulator step failed: ${String(e)}`,
                  })
              )
            );
            isFirstUserTurn = false;
            messages.push({ role: MessageRole.User, content: userResponse });
            if (STOP_TOKENS.some((t) => userResponse.includes(t))) {
              terminationReason = TerminationReason.UserStop;
              done = true;
            } else {
              role = Role.Agent;
            }
            break;
          }
          case Role.Agent: {
            const output = yield* model.generate(messages, genConfig);
            totalGenerationTimeMs += output.generationTimeMs ?? 0;
            if (output.usage) {
              accUsage.inputTokens += output.usage.inputTokens ?? 0;
              accUsage.outputTokens += output.usage.outputTokens ?? 0;
              accUsage.totalTokens += output.usage.totalTokens ?? 0;
              accUsage.reasoningTokens += output.usage.reasoningTokens ?? 0;
              accUsage.totalCost += output.usage.totalCost ?? 0;
            }
            const assistantMsg = output.message;
            messages.push(assistantMsg);
            const toolCalls = assistantMsg.toolCalls ?? [];
            if (toolCalls.length > 0) {
              role = Role.Env;
            } else {
              respondMessages.push(assistantMsg.content);
              role = Role.User;
            }
            break;
          }
          case Role.Env: {
            const lastAssistant = [...messages]
              .toReversed()
              .find((m) => m.role === MessageRole.Assistant);
            const toolCalls = lastAssistant?.toolCalls ?? [];
            for (const tc of toolCalls) {
              const observation = executeToolCall(data, tc);
              messages.push({
                role: MessageRole.Tool,
                content: observation,
                toolCallId: tc.id,
              });
            }
            role = Role.Agent;
            break;
          }
          default: {
            role satisfies never;
            break;
          }
        }
        stepCount += 1;
      }
      const finalUsage: ModelUsage = {
        inputTokens: accUsage.inputTokens,
        outputTokens: accUsage.outputTokens,
        totalTokens: accUsage.totalTokens,
        reasoningTokens: accUsage.reasoningTokens,
        totalCost: accUsage.totalCost,
      };
      return {
        sample: {
          ...state.sample,
          metadata: {
            ...state.sample.metadata,
            task,
            agentData: data,
            terminationReason,
            stepCount,
          },
        },
        messages,
        output: {
          completion: respondMessages.join("\n"),
          message: {
            role: MessageRole.Assistant,
            content: respondMessages.join("\n"),
          },
          usage: finalUsage,
          generationTimeMs: totalGenerationTimeMs,
        },
        completed: true,
      };
    });
}

function lastAssistantText(messages: readonly ChatMessage[]): string {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m?.role === MessageRole.Assistant) {
      return m.content;
    }
  }
  return "";
}

function executeToolCall(data: AirlineData, tc: ToolCall): string {
  const parsed = Either.try((): unknown => JSON.parse(tc.function.arguments));
  if (Either.isLeft(parsed) || !isRecord(parsed.right)) {
    return `Error: invalid JSON arguments: ${tc.function.arguments}`;
  }
  return invokeTool(data, tc.function.name, parsed.right);
}
