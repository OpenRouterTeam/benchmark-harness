import { FetchHttpClient } from "effect/http";
import type { HttpClient } from "effect/http/HttpClient";
import type { Layer } from "effect/Layer";
import { provide, succeed } from "effect/Layer";

const liveFetchLayer = succeed(
  FetchHttpClient.Fetch,
  (input: string | URL | Request, init?: RequestInit) =>
    globalThis.fetch(input, init)
);

export const fetchHttpClientLayer: Layer<HttpClient> =
  FetchHttpClient.layer.pipe(provide(liveFetchLayer));
