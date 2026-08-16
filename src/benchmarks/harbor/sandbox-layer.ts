import type { Layer } from "effect/Layer";

import { makeHttpSandboxLayer } from "./http-sandbox";
import type { ModalSandboxConfig } from "./modal-sandbox";
import { makeModalSandboxLayer } from "./modal-sandbox";
import type { SandboxSession } from "./sandbox";

export const SANDBOX_BACKEND_ENV = "BENCH_SANDBOX_BACKEND";
export const SANDBOX_HTTP_URL_ENV = "BENCH_SANDBOX_HTTP_URL";
export const SANDBOX_HTTP_TOKEN_ENV = "BENCH_SANDBOX_HTTP_TOKEN";
export const SANDBOX_HTTP_HOST_SUFFIXES_ENV =
  "BENCH_SANDBOX_HTTP_HOST_SUFFIXES";

export type SandboxBackend = "modal" | "http";

export function makeHarborSandboxLayer(
  config: ModalSandboxConfig,
  env: Readonly<Record<string, string | undefined>> = process.env
): Layer<SandboxSession> {
  const backend = env[SANDBOX_BACKEND_ENV] ?? "modal";
  if (backend === "modal") {
    return makeModalSandboxLayer(config);
  }
  if (backend !== "http") {
    throw new Error(
      `Unknown ${SANDBOX_BACKEND_ENV} value "${backend}" (expected "modal" or "http")`
    );
  }
  const baseUrl = env[SANDBOX_HTTP_URL_ENV];
  const authToken = env[SANDBOX_HTTP_TOKEN_ENV];
  if (baseUrl === undefined || baseUrl === "") {
    throw new Error(
      `${SANDBOX_HTTP_URL_ENV} must be set when ${SANDBOX_BACKEND_ENV}=http`
    );
  }
  if (authToken === undefined || authToken === "") {
    throw new Error(
      `${SANDBOX_HTTP_TOKEN_ENV} must be set when ${SANDBOX_BACKEND_ENV}=http`
    );
  }
  const allowedHostSuffixes = (env[SANDBOX_HTTP_HOST_SUFFIXES_ENV] ?? "")
    .split(",")
    .map((suffix) => suffix.trim())
    .filter((suffix) => suffix !== "");
  return makeHttpSandboxLayer({ baseUrl, authToken, allowedHostSuffixes });
}
