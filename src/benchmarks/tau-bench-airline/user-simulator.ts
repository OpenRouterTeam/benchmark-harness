import {
  HttpClient,
  HttpClientError,
  HttpClientRequest,
} from "@effect/platform";
import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { catchAll, fail, gen } from "effect/Effect";
import { fixed, passthrough, whileInput } from "effect/Schedule";

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
  recordGenerationId,
  withAuxiliaryUsage,
} from "../../runtime/generation-ids";
import {
  buildResponseCacheSalt,
  getCurrentCallSalt,
  getCurrentEpoch,
  getCurrentRetryAttempt,
  RESPONSE_CACHE_HEADER,
  RESPONSE_CACHE_SALT_FIELD,
  RESPONSE_CACHE_SOURCE_ID_HEADER,
  RESPONSE_CACHE_STATUS_HEADER,
  RESPONSE_CACHE_STATUS_HIT,
  RESPONSE_CACHE_TTL_HEADER,
  RESPONSE_CACHE_TTL_SECONDS,
} from "../../runtime/response-cache";
import { retrySalted, withRetryAttemptLogging } from "../../runtime/retry";
import type { UserModelConfig } from "./types";
import { USER_SIM_GUIDELINES } from "./user-sim-guidelines";

const ChatCompletionResponseSchema = z.object({
  id: z.string().nullish(),
  choices: z.array(
    z.object({
      message: z.object({
        content: z.string(),
        reasoning_details: ReasoningDetailsSchema.optional(),
      }),
    })
  ),
});

const USER_FALLBACK_MODEL = "openai/gpt-5.4-mini";

class UserSimError extends TaggedError("UserSimError")<{
  readonly message: string;
  readonly retryable?: boolean;
}> {}

type SimError = UserSimError | HttpClientError.HttpClientError;

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

interface UserMessage {
  readonly role: "system" | "user" | "assistant";
  readonly content: string;
  readonly reasoning_details?: ReasoningDetails;
}

interface UserModelResponse {
  readonly content: string;
  readonly reasoningDetails?: ReasoningDetails;
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
    this.messages.length = 0;
    this.messages.push(
      { role: "system", content: buildUserSystemPrompt(scenarioInstructions) },
      { role: "user", content: firstAgentMessage }
    );
  }
  generateInitial(): Effect<string, SimError, HttpClient.HttpClient> {
    return this.callModel();
  }
  step(agentMessage: string): Effect<string, SimError, HttpClient.HttpClient> {
    this.messages.push({ role: "user", content: agentMessage });
    return this.callModel();
  }
  private callModel(): Effect<string, SimError, HttpClient.HttpClient> {
    const callModelOnce = this.callModelOnce;
    const messages = this.messages;
    const config = this.config;
    return gen(function* () {
      const response = yield* retrySalted(
        withAuxiliaryUsage(callModelOnce(config.model)),
        USER_SIM_RESPONSE_RETRY_SCHEDULE
      ).pipe(
        catchAll(() =>
          retrySalted(
            withAuxiliaryUsage(callModelOnce(USER_FALLBACK_MODEL)),
            USER_SIM_RESPONSE_RETRY_SCHEDULE
          )
        )
      );
      messages.push({
        role: "assistant",
        content: response.content,
        ...(hasReasoningDetails(response.reasoningDetails) && {
          reasoning_details: response.reasoningDetails,
        }),
      });
      return response.content;
    });
  }
  private readonly callModelOnce = (
    model: string
  ): Effect<UserModelResponse, SimError, HttpClient.HttpClient> => {
    const { baseUrl, config, messages } = this;
    return gen(function* () {
      const epoch = yield* getCurrentEpoch;
      const retryAttempt = yield* getCurrentRetryAttempt;
      const callSalt = yield* getCurrentCallSalt;
      const cacheSalt = buildResponseCacheSalt(
        config.sessionId,
        epoch,
        retryAttempt,
        callSalt
      );
      const request = HttpClientRequest.post(
        `${baseUrl}/chat/completions`
      ).pipe(
        HttpClientRequest.setHeaders({
          Authorization: `Bearer ${config.apiKey}`,
          "Content-Type": "application/json",
          "HTTP-Referer": BENCH_HARNESS_APP_REFERRER,
          "X-OpenRouter-Title": BENCH_HARNESS_APP_TITLE,
          [RESPONSE_CACHE_HEADER]: "true",
          [RESPONSE_CACHE_TTL_HEADER]: `${RESPONSE_CACHE_TTL_SECONDS}`,
          ...(config.sessionId !== undefined && {
            "x-session-id": config.sessionId,
          }),
        }),
        HttpClientRequest.bodyUnsafeJson({
          model,
          messages,
          temperature: 0,
          ...(cacheSalt !== undefined && {
            [RESPONSE_CACHE_SALT_FIELD]: cacheSalt,
          }),
        })
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
        const hasInvalidMessageContent = parsed.left.issues.some(
          (issue) =>
            issue.path.length === 4 &&
            issue.path[0] === "choices" &&
            issue.path[1] === 0 &&
            issue.path[2] === "message" &&
            issue.path[3] === "content"
        );
        return yield* fail(
          new UserSimError({
            message: `User simulator response parse error: ${parsed.left.message}`,
            retryable: hasInvalidMessageContent,
          })
        );
      }
      const isCacheHit =
        response.headers[RESPONSE_CACHE_STATUS_HEADER] ===
        RESPONSE_CACHE_STATUS_HIT;
      const cacheSourceId = response.headers[RESPONSE_CACHE_SOURCE_ID_HEADER];
      const hasSourceId = isCacheHit && cacheSourceId !== undefined;
      yield* recordGenerationId(
        hasSourceId ? cacheSourceId : parsed.right.id,
        isCacheHit,
        hasSourceId
      );
      const message = parsed.right.choices[0]?.message;
      const content = message?.content ?? "";
      return {
        content,
        ...(message?.reasoning_details !== undefined && {
          reasoningDetails: message?.reasoning_details,
        }),
      };
    });
  };
}
