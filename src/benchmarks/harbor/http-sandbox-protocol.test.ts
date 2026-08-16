import { describe, expect, it } from "bun:test";

import { assertLeft, assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import {
  CreateSandboxRequestSchema,
  decodeHttpSandboxId,
  encodeHttpSandboxId,
  ExecStatusResponseSchema,
} from "./http-sandbox-protocol";

describe("encodeHttpSandboxId / decodeHttpSandboxId", () => {
  it("round-trips a host URL and local id", () => {
    const encoded = encodeHttpSandboxId("http://10.0.0.5:8080", "sbx-abc123");

    const decoded = decodeHttpSandboxId(encoded);

    expect(decoded).toEqual({
      hostUrl: "http://10.0.0.5:8080",
      localId: "sbx-abc123",
    });
  });

  it("returns undefined for an id without a separator", () => {
    expect(decodeHttpSandboxId("sbx-abc123")).toBeUndefined();
  });

  it("returns undefined for an id with an empty local part", () => {
    expect(decodeHttpSandboxId("http://10.0.0.5:8080#")).toBeUndefined();
  });

  it("returns undefined when the host part is not an http URL", () => {
    expect(decodeHttpSandboxId("sb-modal-id#local")).toBeUndefined();
  });
});

describe("CreateSandboxRequestSchema", () => {
  it("accepts a full request", () => {
    const result = parseSchema(CreateSandboxRequestSchema, {
      imageTag: "docker.io/library/python:3.12",
      imageBuildSteps: ["RUN apt-get update"],
      timeoutSec: 5400,
      cpus: 2,
      memoryMb: 8192,
      allowInternet: false,
      workdir: "/app",
      command: ["sleep", "infinity"],
    });

    assertRight(result);
    expect(result.right.imageTag).toBe("docker.io/library/python:3.12");
  });

  it("rejects an empty command", () => {
    const result = parseSchema(CreateSandboxRequestSchema, {
      imageTag: "python:3.12",
      timeoutSec: 60,
      cpus: 1,
      memoryMb: 512,
      allowInternet: true,
      workdir: "/",
      command: [],
    });

    assertLeft(result);
  });

  it("rejects a non-integer timeout", () => {
    const result = parseSchema(CreateSandboxRequestSchema, {
      imageTag: "python:3.12",
      timeoutSec: 1.5,
      cpus: 1,
      memoryMb: 512,
      allowInternet: true,
      workdir: "/",
      command: ["sleep", "infinity"],
    });

    assertLeft(result);
  });
});

describe("ExecStatusResponseSchema", () => {
  it("accepts a running status without output fields", () => {
    const result = parseSchema(ExecStatusResponseSchema, {
      status: "running",
    });

    assertRight(result);
  });

  it("requires output fields when done", () => {
    const result = parseSchema(ExecStatusResponseSchema, { status: "done" });

    assertLeft(result);
  });
});
