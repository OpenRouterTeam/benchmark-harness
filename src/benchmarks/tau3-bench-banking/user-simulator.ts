import {
  HttpClient,
  HttpClientError,
  HttpClientRequest,
} from "@effect/platform";
import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { fail, gen, suspend } from "effect/Effect";

import type { ToolDefinition } from "../../harness/core";
import type { ReasoningDetails } from "../../harness/reasoning-details";
import {
  ReasoningDetailsSchema,
  hasReasoningDetails,
} from "../../harness/reasoning-details";
import { Either } from "../../internal/either";
import { parseSchema, z } from "../../internal/zod";
import {
  BENCH_HARNESS_APP_REFERRER,
  BENCH_HARNESS_APP_TITLE,
} from "../../providers/openrouter-model";
import {
  buildResponseCacheSalt,
  getCurrentCallSalt,
  getCurrentEpoch,
  getCurrentRetryAttempt,
  RESPONSE_CACHE_HEADER,
  RESPONSE_CACHE_SALT_FIELD,
  RESPONSE_CACHE_TTL_HEADER,
  RESPONSE_CACHE_TTL_SECONDS,
} from "../../runtime/response-cache";
import type { UserModelConfig } from "./types";
import {
  USER_SIM_GUIDELINES,
  USER_SIM_GUIDELINES_TOOLS,
} from "./user-sim-guidelines";

class UserSimError extends TaggedError("UserSimError")<{
  readonly message: string;
}> {}

type SimError = UserSimError | HttpClientError.HttpClientError;

const ToolCallFunctionSchema = z.object({
  name: z.string(),
  arguments: z.string(),
});

const ChatCompletionToolCallSchema = z.object({
  id: z.string(),
  type: z.literal("function"),
  function: ToolCallFunctionSchema,
});

const ChatCompletionChoiceSchema = z.object({
  message: z.object({
    content: z.string().nullish(),
    tool_calls: z.array(ChatCompletionToolCallSchema).optional(),
    reasoning_details: ReasoningDetailsSchema.optional(),
  }),
});

const ChatCompletionResponseSchema = z.object({
  choices: z.array(ChatCompletionChoiceSchema),
});

type SimulatorTurn = TextTurn | ToolCallsTurn;

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

interface ToolCall {
  readonly id: string;
  readonly type: "function";
  readonly function: {
    readonly name: string;
    readonly arguments: string;
  };
}

interface UserMessage {
  readonly role: "system" | "user" | "assistant" | "tool";
  readonly content?: string;
  readonly tool_call_id?: string;
  readonly tool_calls?: ToolCall[];
  readonly reasoning_details?: ReasoningDetails;
}

function buildUserSystemPrompt(
  scenarioInstructions: string,
  useTools: boolean
): string {
  const guidelines = useTools ? USER_SIM_GUIDELINES_TOOLS : USER_SIM_GUIDELINES;
  return `${guidelines}\n\n<scenario>\n${scenarioInstructions}\n</scenario>`;
}

export class UserSimulator {
  private readonly messages: UserMessage[] = [];
  private readonly config: UserModelConfig;
  private readonly baseUrl: string;
  constructor(config: UserModelConfig) {
    this.config = config;
    const raw = config.baseUrl ?? "https://openrouter.ai";
    const trimmed = raw.replace(/\/+$/, "");
    this.baseUrl = trimmed.endsWith("/api/v1") ? trimmed : `${trimmed}/api/v1`;
  }
  reset(scenarioInstructions: string, firstAgentMessage: string): void {
    const useTools = this.availableTools.length > 0;
    this.messages.length = 0;
    this.messages.push(
      {
        role: "system",
        content: buildUserSystemPrompt(scenarioInstructions, useTools),
      },
      { role: "user", content: firstAgentMessage }
    );
  }
  generateInitial(): Effect<SimulatorTurn, SimError, HttpClient.HttpClient> {
    return this.callModel();
  }
  step(
    agentMessage: string
  ): Effect<SimulatorTurn, SimError, HttpClient.HttpClient> {
    return suspend(() => {
      this.messages.push({ role: "user", content: agentMessage });
      return this.callModel();
    });
  }
  continueAfterTools(): Effect<SimulatorTurn, SimError, HttpClient.HttpClient> {
    return this.callModel();
  }
  setAvailableTools(toolDefs: readonly ToolDefinition[]): void {
    this.availableTools = toolDefs;
  }
  private availableTools: readonly ToolDefinition[] = [];
  addToolResult(toolCallId: string, content: string): void {
    this.messages.push({
      role: "tool",
      content,
      tool_call_id: toolCallId,
    });
  }
  private callModel(): Effect<SimulatorTurn, SimError, HttpClient.HttpClient> {
    return this.callModelOnce(this.config.model);
  }
  private callModelOnce(
    model: string
  ): Effect<SimulatorTurn, SimError, HttpClient.HttpClient> {
    return gen(this, function* (this: UserSimulator) {
      const epoch = yield* getCurrentEpoch;
      const retryAttempt = yield* getCurrentRetryAttempt;
      const callSalt = yield* getCurrentCallSalt;
      const cacheSalt = buildResponseCacheSalt(
        this.config.sessionId,
        epoch,
        retryAttempt,
        callSalt
      );
      const requestBody: Record<string, unknown> = {
        model,
        messages: this.messages,
        temperature: 0,
        ...(this.config.userReasoningEffort !== undefined && {
          reasoning_effort: this.config.userReasoningEffort,
        }),
        ...(cacheSalt !== undefined && {
          [RESPONSE_CACHE_SALT_FIELD]: cacheSalt,
        }),
      };
      if (this.availableTools.length > 0) {
        requestBody.tools = [...this.availableTools];
      }
      const request = HttpClientRequest.post(
        `${this.baseUrl}/chat/completions`
      ).pipe(
        HttpClientRequest.setHeaders({
          Authorization: `Bearer ${this.config.apiKey}`,
          "Content-Type": "application/json",
          "HTTP-Referer": BENCH_HARNESS_APP_REFERRER,
          "X-OpenRouter-Title": BENCH_HARNESS_APP_TITLE,
          [RESPONSE_CACHE_HEADER]: "true",
          [RESPONSE_CACHE_TTL_HEADER]: `${RESPONSE_CACHE_TTL_SECONDS}`,
          ...(this.config.sessionId !== undefined && {
            "x-session-id": this.config.sessionId,
          }),
        }),
        HttpClientRequest.bodyUnsafeJson(requestBody)
      );
      const client = yield* HttpClient.HttpClient;
      const response = yield* client.execute(request);
      if (response.status < 200 || response.status >= 300) {
        const text = yield* response.text;
        return yield* fail(
          new UserSimError({
            message: `User simulator HTTP ${response.status}: ${text}`,
          })
        );
      }
      const json: unknown = yield* response.json;
      const parsed = parseSchema(ChatCompletionResponseSchema, json);
      if (Either.isLeft(parsed)) {
        return yield* fail(
          new UserSimError({
            message: `User simulator response parse error: ${parsed.left.message}`,
          })
        );
      }
      const choice = parsed.right.choices[0];
      if (!choice) {
        return yield* fail(
          new UserSimError({
            message: "User simulator: no choices in response",
          })
        );
      }
      const { message } = choice;
      const content = message.content ?? "";
      if (message.tool_calls && message.tool_calls.length > 0) {
        const toolCalls = message.tool_calls.map((tc) => ({
          id: tc.id,
          name: tc.function.name,
          arguments: tc.function.arguments,
        }));
        const assistantMsg: UserMessage = {
          role: "assistant",
          content: content.length > 0 ? content : undefined,
          tool_calls: message.tool_calls,
          ...(hasReasoningDetails(message.reasoning_details) && {
            reasoning_details: message.reasoning_details,
          }),
        };
        this.messages.push(assistantMsg);
        return { kind: "toolCalls", calls: toolCalls };
      }
      this.messages.push({
        role: "assistant",
        content,
        ...(hasReasoningDetails(message.reasoning_details) && {
          reasoning_details: message.reasoning_details,
        }),
      });
      return { kind: "text", content };
    });
  }
}
