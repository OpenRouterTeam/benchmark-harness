import { describe, expect, it } from "bun:test";

import {
  SANDBOX_BACKEND_ENV,
  SANDBOX_HTTP_TOKEN_ENV,
  SANDBOX_HTTP_URL_ENV,
} from "../harbor/sandbox-layer";
import { makeTerminalBenchSandboxLayer } from "./sandbox-layer";

const MODAL_CONFIG = { environment: "main" };

describe("makeTerminalBenchSandboxLayer", () => {
  it("defaults to the Modal backend when no env is set", () => {
    const layer = makeTerminalBenchSandboxLayer(MODAL_CONFIG, {});

    expect(layer).toBeDefined();
  });

  it("builds the HTTP backend when configured", () => {
    const layer = makeTerminalBenchSandboxLayer(MODAL_CONFIG, {
      [SANDBOX_BACKEND_ENV]: "http",
      [SANDBOX_HTTP_URL_ENV]: "http://sandbox-lb.internal",
      [SANDBOX_HTTP_TOKEN_ENV]: "token",
    });

    expect(layer).toBeDefined();
  });

  it("throws when the HTTP backend has no URL", () => {
    expect(() =>
      makeTerminalBenchSandboxLayer(MODAL_CONFIG, {
        [SANDBOX_BACKEND_ENV]: "http",
        [SANDBOX_HTTP_TOKEN_ENV]: "token",
      })
    ).toThrow(SANDBOX_HTTP_URL_ENV);
  });

  it("throws when the HTTP backend has no token", () => {
    expect(() =>
      makeTerminalBenchSandboxLayer(MODAL_CONFIG, {
        [SANDBOX_BACKEND_ENV]: "http",
        [SANDBOX_HTTP_URL_ENV]: "http://sandbox-lb.internal",
      })
    ).toThrow(SANDBOX_HTTP_TOKEN_ENV);
  });

  it("throws on an unknown backend value", () => {
    expect(() =>
      makeTerminalBenchSandboxLayer(MODAL_CONFIG, {
        [SANDBOX_BACKEND_ENV]: "kubernetes",
      })
    ).toThrow("kubernetes");
  });
});
