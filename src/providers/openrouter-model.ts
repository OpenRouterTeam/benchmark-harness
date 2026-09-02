import { gen, map } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect, provide } from "effect/Layer";

import { Model } from "../harness/model";
import { definedValues } from "../internal/guards";
import type { RetryConfig } from "../runtime/retry";
import {
  messagesToResponses,
  responsesTurnToModelOutput,
  toolDefinitionToResponses,
} from "./messages-to-responses";
import { makeResponsesModelLayer, ResponsesModel } from "./responses-model";

export {
  BENCH_HARNESS_APP_REFERRER,
  BENCH_HARNESS_APP_TITLE,
} from "./app-identity";

export interface OpenRouterModelConfig {
  readonly model: string;
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly sessionId?: string;
  readonly retry?: RetryConfig;
  readonly traceHeaders?: Readonly<Record<string, string>>;
}

export function normalizeBaseUrl(baseUrl: string): string {
  const trimmed = baseUrl.replace(/\/+$/u, "");
  return trimmed.endsWith("/api/v1") ? trimmed : `${trimmed}/api/v1`;
}

export function makeOpenRouterModelLayer(
  config: OpenRouterModelConfig
): Layer<Model> {
  const responsesLayer = makeResponsesModelLayer(
    definedValues({
      model: config.model,
      apiKey: config.apiKey,
      ...(config.baseUrl !== undefined && {
        baseUrl: normalizeBaseUrl(config.baseUrl),
      }),
      sessionId: config.sessionId,
      retry: config.retry,
      traceHeaders: config.traceHeaders,
    })
  );
  return effect(Model)(
    gen(function* () {
      const responsesModel = yield* ResponsesModel;
      return Model.of({
        generate: (messages, generateConfig) => {
          const { tools, ...rest } = generateConfig;
          return responsesModel
            .generate(messagesToResponses(messages), {
              ...rest,
              ...(tools !== undefined && {
                tools: tools.map(toolDefinitionToResponses),
              }),
            })
            .pipe(map(responsesTurnToModelOutput));
        },
      });
    })
  ).pipe(provide(responsesLayer));
}
