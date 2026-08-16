import { describe, expect, it } from "bun:test";

import {
  makeHarborSandboxLayer,
  SANDBOX_BACKEND_ENV,
  SANDBOX_HTTP_TOKEN_ENV,
  SANDBOX_HTTP_URL_ENV,
} from "./sandbox-layer";

const MODAL_CONFIG = { appName: "openrouter-test" };

describe("makeHarborSandboxLayer", () => {
  it("defaults to the Modal backend when no env is set", () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {});

    expect(layer).toBeDefined();
  });

  it("builds the HTTP backend when configured", () => {
    const layer = makeHarborSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "http",
      [SANDBOX_HTTP_URL_ENV]: "http://sandbox-lb.internal",
      [SANDBOX_HTTP_TOKEN_ENV]: "token",
    });

    expect(layer).toBeDefined();
  });

  it("throws when the HTTP backend has no URL", () => {
    expect(() =>
      makeHarborSandboxLayer(MODAL_CONFIG, {
        [SANDBOX_BACKEND_ENV]: "http",
        [SANDBOX_HTTP_TOKEN_ENV]: "token",
      })
    ).toThrow(SANDBOX_HTTP_URL_ENV);
  });

  it("throws when the HTTP backend has no token", () => {
    expect(() =>
      makeHarborSandboxLayer(MODAL_CONFIG, {
        [SANDBOX_BACKEND_ENV]: "http",
        [SANDBOX_HTTP_URL_ENV]: "http://sandbox-lb.internal",
      })
    ).toThrow(SANDBOX_HTTP_TOKEN_ENV);
  });

  it("throws on an unknown backend value", () => {
    expect(() =>
      makeHarborSandboxLayer(MODAL_CONFIG, {
        [SANDBOX_BACKEND_ENV]: "kubernetes",
      })
    ).toThrow("kubernetes");
  });
});
