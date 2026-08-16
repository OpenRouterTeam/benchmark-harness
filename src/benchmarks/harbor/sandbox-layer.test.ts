import { describe, expect, it } from "bun:test";

import { runPromiseExit, scoped } from "effect/Effect";
import type { Exit } from "effect/Exit";
import type { Layer } from "effect/Layer";
import { build } from "effect/Layer";

import type { SandboxSession } from "./sandbox";
import {
  makeHarborSandboxLayer,
  SANDBOX_BACKEND_ENV,
  SANDBOX_HTTP_ALLOW_INSECURE_ENV,
  SANDBOX_HTTP_TOKEN_ENV,
  SANDBOX_HTTP_URL_ENV,
} from "./sandbox-layer";

const MODAL_CONFIG = { appName: "openrouter-test" };

function buildLayer(
  layer: Layer<SandboxSession, Error>
): Promise<Exit<unknown, Error>> {
  return runPromiseExit(scoped(build(layer)));
}

function causeText(exit: Exit<unknown, Error>): string {
  return exit._tag === "Failure" ? String(exit.cause) : "";
}

describe("makeHarborSandboxLayer", () => {
  it("defaults to the Modal backend when no env is set", () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {});

    expect(layer).toBeDefined();
  });

  it("defaults to the Modal backend when the backend env is empty", () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "",
    });

    expect(layer).toBeDefined();
  });

  it("builds the HTTP backend for an https URL", async () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "http",
      [SANDBOX_HTTP_URL_ENV]: "https://sandbox-lb.internal",
      [SANDBOX_HTTP_TOKEN_ENV]: "token",
    });

    const exit = await buildLayer(layer);

    expect(exit._tag).toBe("Success");
  });

  it("builds the HTTP backend for a plain-http URL when insecure is allowed", async () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "http",
      [SANDBOX_HTTP_URL_ENV]: "http://sandbox-lb.internal",
      [SANDBOX_HTTP_TOKEN_ENV]: "token",
      [SANDBOX_HTTP_ALLOW_INSECURE_ENV]: "1",
    });

    const exit = await buildLayer(layer);

    expect(exit._tag).toBe("Success");
  });

  it("fails for a plain-http URL on a public host even when insecure is allowed", async () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "http",
      [SANDBOX_HTTP_URL_ENV]: "http://sandbox-lb.example.com",
      [SANDBOX_HTTP_TOKEN_ENV]: "token",
      [SANDBOX_HTTP_ALLOW_INSECURE_ENV]: "1",
    });

    const exit = await buildLayer(layer);

    expect(exit._tag).toBe("Failure");
    expect(causeText(exit)).toContain("non-private host");
  });

  it("builds the HTTP backend for a plain-http localhost URL when insecure is allowed", async () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "http",
      [SANDBOX_HTTP_URL_ENV]: "http://localhost:8700",
      [SANDBOX_HTTP_TOKEN_ENV]: "token",
      [SANDBOX_HTTP_ALLOW_INSECURE_ENV]: "1",
    });

    const exit = await buildLayer(layer);

    expect(exit._tag).toBe("Success");
  });

  it("fails for a plain-http URL when insecure is not allowed", async () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "http",
      [SANDBOX_HTTP_URL_ENV]: "http://sandbox-lb.internal",
      [SANDBOX_HTTP_TOKEN_ENV]: "token",
    });

    const exit = await buildLayer(layer);

    expect(exit._tag).toBe("Failure");
    expect(causeText(exit)).toContain(SANDBOX_HTTP_ALLOW_INSECURE_ENV);
  });

  it("fails when the HTTP backend has no URL", async () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "http",
      [SANDBOX_HTTP_TOKEN_ENV]: "token",
    });

    const exit = await buildLayer(layer);

    expect(exit._tag).toBe("Failure");
    expect(causeText(exit)).toContain(SANDBOX_HTTP_URL_ENV);
  });

  it("fails when the HTTP backend has no token", async () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "http",
      [SANDBOX_HTTP_URL_ENV]: "https://sandbox-lb.internal",
    });

    const exit = await buildLayer(layer);

    expect(exit._tag).toBe("Failure");
    expect(causeText(exit)).toContain(SANDBOX_HTTP_TOKEN_ENV);
  });

  it("fails on an unknown backend value", async () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "kubernetes",
    });

    const exit = await buildLayer(layer);

    expect(exit._tag).toBe("Failure");
    expect(causeText(exit)).toContain("kubernetes");
  });
});
