import { FetchHttpClient } from "effect/http";
import type { HttpClient } from "effect/http/HttpClient";
import type { Layer } from "effect/Layer";
import { provide, succeed } from "effect/Layer";

const CREDENTIAL_HEADER_PREFIXES = ["authorization", "cf-access-"] as const;

export function carriesCredentials(
  input: string | URL | Request,
  init: RequestInit | undefined
): boolean {
  const names = [
    ...(input instanceof Request ? input.headers.keys() : []),
    ...new Headers(init?.headers).keys(),
  ];
  return names.some((name) =>
    CREDENTIAL_HEADER_PREFIXES.some((prefix) =>
      name.toLowerCase().startsWith(prefix)
    )
  );
}

const liveFetchLayer = succeed(
  FetchHttpClient.Fetch,
  (input: string | URL | Request, init?: RequestInit) =>
    globalThis.fetch(
      input,
      carriesCredentials(input, init) ? { ...init, redirect: "error" } : init
    )
);

export const fetchHttpClientLayer: Layer<HttpClient> =
  FetchHttpClient.layer.pipe(provide(liveFetchLayer));
