import {
  FetchHttpClient,
  HttpClient,
  HttpClientRequest,
} from "@effect/platform";
import { map } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect as layerEffect, provide as layerProvide } from "effect/Layer";

import { normalizeBaseUrl } from "../providers/openrouter-model";

const DEFAULT_BASE_URL = "https://openrouter.ai/api/v1";

export function applyTraceHeaders(
  request: HttpClientRequest.HttpClientRequest,
  apiPrefix: string,
  traceHeaders: Readonly<Record<string, string>>
): HttpClientRequest.HttpClientRequest {
  return request.url === apiPrefix || request.url.startsWith(`${apiPrefix}/`)
    ? HttpClientRequest.setHeaders(request, traceHeaders)
    : request;
}

export function makeHttpClientLayer(opts: {
  readonly traceHeaders?: Readonly<Record<string, string>>;
  readonly baseUrl?: string;
}): Layer<HttpClient.HttpClient> {
  const traceHeaders = opts.traceHeaders;
  if (traceHeaders === undefined) {
    return FetchHttpClient.layer;
  }
  const apiPrefix = normalizeBaseUrl(opts.baseUrl ?? DEFAULT_BASE_URL);
  return layerEffect(
    HttpClient.HttpClient,
    map(HttpClient.HttpClient, (client) =>
      HttpClient.mapRequest(client, (request) =>
        applyTraceHeaders(request, apiPrefix, traceHeaders)
      )
    )
  ).pipe(layerProvide(FetchHttpClient.layer));
}
