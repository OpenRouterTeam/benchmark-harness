import { HttpClient } from "@effect/platform";
import type { Semaphore } from "effect/Effect";
import { gen, mapError, provideService } from "effect/Effect";

import type {
  ModelMessage,
  ModelUsage,
  TaskState,
  ToolDefinition,
} from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import { Either } from "../../internal/either";
import { definedValues } from "../../internal/guards";
import { parseSchema } from "../../internal/zod";
import type { ResponsesModelService } from "../../providers/responses-model";
import {
  buildBankingAgentSystemPrompt,
  DEFAULT_FIRST_AGENT_MESSAGE,
} from "./agent-prompts";
import {
  ensureAllBankingDocuments,
  ensureBankingDocuments,
  getAllBankingDocuments,
} from "./documents";
import {
  applyInitialState,
  ensureBankingData,
  ensureBankingTasks,
  loadBankingData,
} from "./environment";
import type { PredictedToolCall } from "./evaluator";
import { makeBankingRetrievalTools } from "./retrieval";
import { DEFAULT_BANKING_RETRIEVAL_CONFIG } from "./retrieval-config";
import { BANKING_TOOL_DEFINITIONS } from "./tools/definitions";
import {
  invokeBankingAgentTool,
  registerInitialDiscoverableTools,
} from "./tools/handlers-meta";
import {
  invokeBankingUserTool,
  USER_PERMANENT_TOOL_DEFINITIONS,
} from "./tools/handlers-user";
import { parseToolArguments } from "./tools/helpers";
import { makeBankingEnvState } from "./tools/registry";
import type { SolverOpts } from "./types";
import { deriveReadLogAllowlist, Tau3TaskSchema } from "./types";
import { UserSimulator } from "./user-simulator";

const STOP_TOKENS = ["###STOP###", "###TRANSFER###", "###OUT-OF-SCOPE###"];

const MAX_STEPS = 200;

const MAX_ERRORS = 10;

const BANKING_TEMPERATURE = 0;

export const TerminationReason = {
  UserStop: "USER_STOP",
  AgentStop: "AGENT_STOP",
  MaxSteps: "MAX_STEPS",
  TooManyErrors: "TOO_MANY_ERRORS",
} as const;

const Role = {
  User: "user",
  Agent: "agent",
  Env: "env",
} as const;

type Role = (typeof Role)[keyof typeof Role];

function lastAssistantText(messages: readonly ModelMessage[]): string {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i]!.role === MessageRole.Assistant) {
      return messages[i]!.content;
    }
  }
  return "";
}

export function selectUserToolDefinitions(
  toolNames: readonly string[]
): readonly ToolDefinition[] {
  const grantedToolNames = new Set(toolNames);
  return USER_PERMANENT_TOOL_DEFINITIONS.filter((definition) =>
    grantedToolNames.has(definition.function.name)
  );
}

type UserTurnState = "initial" | "continuing_tools" | "awaiting_reply";

function selectUserTurnEffect(opts: {
  readonly userSim: UserSimulator;
  readonly turnState: UserTurnState;
  readonly messages: readonly ModelMessage[];
}): ReturnType<UserSimulator["step"]> {
  switch (opts.turnState) {
    case "initial": {
      return opts.userSim.generateInitial();
    }
    case "continuing_tools": {
      return opts.userSim.continueAfterTools();
    }
    case "awaiting_reply": {
      return opts.userSim.step(lastAssistantText(opts.messages));
    }
    default: {
      return opts.turnState satisfies never;
    }
  }
}

function isToolErrorResult(result: string): boolean {
  return result.startsWith("Error:");
}

export function bankingSolver({
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
  readonly opts: SolverOpts;
}): SolverService {
  return (state: TaskState) =>
    gen(function* () {
      const userModelConfig = opts?.userModelConfig;
      if (!userModelConfig) {
        return yield* new SolverError({
          message:
            "bankingSolver requires userModelConfig (apiKey + model for user simulator)",
        });
      }
      yield* ensureBankingData(dataFetchLock).pipe(
        provideService(HttpClient.HttpClient, client),
        mapError(
          (e) =>
            new SolverError({
              message: `Failed to load banking data from GitHub: ${String(e)}`,
            })
        )
      );
      yield* ensureBankingTasks(dataFetchLock).pipe(
        provideService(HttpClient.HttpClient, client),
        mapError(
          (e) =>
            new SolverError({
              message: `Failed to load banking tasks from GitHub: ${String(e)}`,
            })
        )
      );
      const taskMeta = state.sample.metadata?.["task"];
      const parseTaskResult = parseSchema(Tau3TaskSchema, taskMeta);
      if (Either.isLeft(parseTaskResult)) {
        return yield* new SolverError({
          message: `Invalid task in metadata: ${String(parseTaskResult.left)}`,
        });
      }
      const task = parseTaskResult.right;
      const retrievalConfig =
        opts?.retrievalConfig ?? DEFAULT_BANKING_RETRIEVAL_CONFIG;
      const requiredDocIds = task.required_documents ?? [];
      if (retrievalConfig === "required_docs") {
        yield* ensureBankingDocuments(requiredDocIds, dataFetchLock).pipe(
          provideService(HttpClient.HttpClient, client),
          mapError(
            (e) =>
              new SolverError({
                message: `Failed to load documents: ${String(e)}`,
              })
          )
        );
      } else {
        yield* ensureAllBankingDocuments(dataFetchLock).pipe(
          provideService(HttpClient.HttpClient, client),
          mapError(
            (e) =>
              new SolverError({
                message: `Failed to load document corpus: ${String(e)}`,
              })
          )
        );
      }
      const retrievalTools = makeBankingRetrievalTools({
        documents: getAllBankingDocuments(),
        retrievalConfig,
      });
      const agentData = loadBankingData();
      applyInitialState(agentData, task);
      const envState = makeBankingEnvState(
        agentData,
        deriveReadLogAllowlist(task)
      );
      registerInitialDiscoverableTools();
      const userToolDefs = selectUserToolDefinitions(task.user_tools ?? []);
      const userSim = new UserSimulator(userModel, userModelConfig);
      userSim.setAvailableTools(userToolDefs);
      userSim.reset(state.sample.input, DEFAULT_FIRST_AGENT_MESSAGE);
      const systemPrompt = buildBankingAgentSystemPrompt({
        requiredDocIds,
        retrievalConfig,
      });
      const messages: ModelMessage[] = [
        { role: MessageRole.System, content: systemPrompt },
        { role: MessageRole.Assistant, content: DEFAULT_FIRST_AGENT_MESSAGE },
      ];
      const respondMessages: string[] = [];
      const predictedUserToolCalls: PredictedToolCall[] = [];
      const genConfig: GenerateConfig = {
        temperature: BANKING_TEMPERATURE,
        tools: [...BANKING_TOOL_DEFINITIONS, ...retrievalTools.definitions],
        ...definedValues(opts.inference),
        ...(opts.endpointId !== undefined && { endpointId: opts.endpointId }),
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
      let numToolErrors = 0;
      let userTurnState: UserTurnState = "initial";
      let terminationReason: string = TerminationReason.MaxSteps;
      let done = false;
      while (!done) {
        if (stepCount >= MAX_STEPS) {
          terminationReason = TerminationReason.MaxSteps;
          break;
        }
        if (numToolErrors >= MAX_ERRORS) {
          terminationReason = TerminationReason.TooManyErrors;
          break;
        }
        switch (role) {
          case Role.User: {
            const userTurnEffect = selectUserTurnEffect({
              userSim,
              turnState: userTurnState,
              messages,
            });
            const userTurn = yield* userTurnEffect.pipe(
              provideService(HttpClient.HttpClient, client),
              mapError(
                (e) =>
                  new SolverError({
                    message: `User simulator step failed: ${String(e)}`,
                  })
              )
            );
            if (userTurn.kind === "toolCalls") {
              for (const tc of userTurn.calls) {
                const args = parseToolArguments(tc.arguments);
                predictedUserToolCalls.push({
                  name: tc.name,
                  arguments: args,
                  requestor: "user",
                });
                const toolResultStr = invokeBankingUserTool(
                  envState,
                  tc.name,
                  args
                );
                if (isToolErrorResult(toolResultStr)) {
                  numToolErrors++;
                }
                userSim.addToolResult(tc.id, toolResultStr);
              }
              userTurnState = "continuing_tools";
            } else {
              userTurnState = "awaiting_reply";
              const userResponse = userTurn.content;
              messages.push({ role: MessageRole.User, content: userResponse });
              if (STOP_TOKENS.some((t) => userResponse.includes(t))) {
                terminationReason = TerminationReason.UserStop;
                done = true;
              } else {
                role = Role.Agent;
              }
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
            const assistantMsg = messages.at(-1);
            const toolCalls = assistantMsg?.toolCalls ?? [];
            for (const tc of toolCalls) {
              const toolName = tc.function.name;
              const toolArgs = parseToolArguments(tc.function.arguments);
              const toolResultStr =
                retrievalTools.invoke(toolName, toolArgs) ??
                invokeBankingAgentTool(envState, toolName, toolArgs);
              if (isToolErrorResult(toolResultStr)) {
                numToolErrors++;
              }
              messages.push({
                role: MessageRole.Tool,
                content: toolResultStr,
                toolCallId: tc.id,
              });
            }
            role = Role.Agent;
            break;
          }
        }
        stepCount++;
      }
      const finalUsage: ModelUsage = {
        inputTokens: accUsage.inputTokens,
        outputTokens: accUsage.outputTokens,
        totalTokens: accUsage.totalTokens,
        reasoningTokens: accUsage.reasoningTokens,
        totalCost: accUsage.totalCost,
      };
      const completion = respondMessages.join("\n");
      return {
        sample: {
          ...state.sample,
          metadata: {
            ...state.sample.metadata,
            task,
            agentData,
            terminationReason,
            stepCount,
            predictedUserToolCalls,
          },
        },
        messages,
        output: {
          completion,
          message: { role: MessageRole.Assistant, content: completion },
          usage: finalUsage,
          generationTimeMs: totalGenerationTimeMs,
        },
        completed: true,
      };
    });
}
