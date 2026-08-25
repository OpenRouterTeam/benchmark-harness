import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { catchAll, gen, map, mapError } from "effect/Effect";
import { fixed, passthrough, whileInput } from "effect/Schedule";

import type { ChatMessage } from "../../harness/core";
import { MessageRole } from "../../harness/core";
import { chatMessagesToResponses } from "../../providers/chat-to-responses";
import type { ResponsesModelService } from "../../providers/responses-model";
import { withAuxiliaryUsage } from "../../runtime/generation-ids";
import { retrySalted, withRetryAttemptLogging } from "../../runtime/retry";
import type { UserModelConfig } from "./types";
import { USER_SIM_GUIDELINES } from "./user-sim-guidelines";

const USER_FALLBACK_MODEL = "openai/gpt-5.4-mini";

class UserSimError extends TaggedError("UserSimError")<{
  readonly message: string;
  readonly retryable?: boolean;
}> {}

type SimError = UserSimError;

const USER_SIM_MAX_RETRIES = 2;

const USER_SIM_RESPONSE_RETRY_SCHEDULE = withRetryAttemptLogging(
  fixed("100 millis").pipe(
    whileInput(
      (error: SimError) =>
        error instanceof UserSimError && error.retryable === true
    ),
    passthrough
  ),
  USER_SIM_MAX_RETRIES
);

function buildUserSystemPrompt(scenarioInstructions: string): string {
  return `${USER_SIM_GUIDELINES}\n\n<scenario>\n${scenarioInstructions}\n</scenario>`;
}

interface UserModelResponse {
  readonly content: string;
  readonly responseItems: readonly Record<string, unknown>[];
}

export class UserSimulator {
  private readonly messages: ChatMessage[] = [];
  private readonly config: UserModelConfig;
  private readonly model: ResponsesModelService;

  constructor(model: ResponsesModelService, config: UserModelConfig) {
    this.model = model;
    this.config = config;
  }

  reset(scenarioInstructions: string, firstAgentMessage: string): void {
    this.messages.length = 0;
    this.messages.push(
      {
        role: MessageRole.System,
        content: buildUserSystemPrompt(scenarioInstructions),
      },
      { role: MessageRole.User, content: firstAgentMessage }
    );
  }

  generateInitial(): Effect<string, SimError> {
    return this.callModel();
  }

  step(agentMessage: string): Effect<string, SimError> {
    this.messages.push({ role: MessageRole.User, content: agentMessage });
    return this.callModel();
  }

  private callModel(): Effect<string, SimError> {
    const response = (model: string) =>
      retrySalted(this.callModelOnce(model), USER_SIM_RESPONSE_RETRY_SCHEDULE);
    return gen(this, function* (this: UserSimulator) {
      const result = yield* response(this.config.model).pipe(
        withAuxiliaryUsage,
        catchAll(() => withAuxiliaryUsage(response(USER_FALLBACK_MODEL)))
      );
      this.messages.push({
        role: MessageRole.Assistant,
        content: result.content,
        ...(result.responseItems.length > 0 && {
          responseItems: result.responseItems,
        }),
      });
      return result.content;
    });
  }

  private callModelOnce(model: string): Effect<UserModelResponse, SimError> {
    return this.model
      .generate(chatMessagesToResponses(this.messages), {
        model,
        temperature: 0,
      })
      .pipe(
        mapError((error) => new UserSimError({ message: error.message })),
        map((turn) => ({
          content: turn.text,
          responseItems: turn.outputItems,
        }))
      );
  }
}
