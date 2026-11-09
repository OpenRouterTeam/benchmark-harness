import { FetchHttpClient } from "effect/http";
import type { HttpClient } from "effect/http/HttpClient";
import type { Layer } from "effect/Layer";
import { provide, succeed } from "effect/Layer";

const REDIRECT_SURVIVING_CREDENTIAL_PREFIX = "cf-access-";

function carriesRedirectSurvivingCredential(
  input: string | URL | Request,
  init: RequestInit | undefined
): boolean {
  const names = [
    ...(input instanceof Request ? input.headers.keys() : []),
    ...new Headers(init?.headers).keys(),
  ];
  return names.some((name) =>
    name.toLowerCase().startsWith(REDIRECT_SURVIVING_CREDENTIAL_PREFIX)
  );
}

export const TEST_carriesRedirectSurvivingCredential =
  carriesRedirectSurvivingCredential;

const liveFetchLayer = succeed(
  FetchHttpClient.Fetch,
  (input: string | URL | Request, init?: RequestInit) =>
    globalThis.fetch(
      input,
      carriesRedirectSurvivingCredential(input, init)
        ? { ...init, redirect: "error" }
        : init
    )
);

export const fetchHttpClientLayer: Layer<HttpClient> =
  FetchHttpClient.layer.pipe(provide(liveFetchLayer));
