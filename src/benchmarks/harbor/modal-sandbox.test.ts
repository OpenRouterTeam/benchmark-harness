import { describe, expect, it } from "bun:test";

import { assertLeft, assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { toSandboxCreateParams } from "./modal-sandbox";
import {
  DEFAULT_MODAL_REGIONS,
  ModalSandboxOptionsSchema,
} from "./modal-schema";
import type { CreateSessionInput } from "./sandbox";

const input: CreateSessionInput = {
  imageTag: "python:3.12",
  timeoutSec: 60,
  cpus: 2,
  memoryMb: 4096,
  allowInternet: true,
  workdir: "/workspace",
  keepAliveCommand: ["sleep", "infinity"],
  uploads: [],
};

describe("toSandboxCreateParams", () => {
  it("pins the sandbox to the configured regions", () => {
    expect(toSandboxCreateParams(["us"], input)).toEqual({
      timeoutMs: 60_000,
      cpu: 2,
      memoryMiB: 4096,
      blockNetwork: false,
      workdir: "/workspace",
      command: ["sleep", "infinity"],
      regions: ["us"],
    });
  });

  it("omits regions when none are configured", () => {
    expect(toSandboxCreateParams(undefined, input)).not.toHaveProperty(
      "regions"
    );
    expect(toSandboxCreateParams([], input)).not.toHaveProperty("regions");
  });
});

describe("ModalSandboxOptionsSchema", () => {
  it("defaults to the main environment pinned to the broad US region", () => {
    const parsed = parseSchema(ModalSandboxOptionsSchema, {});
    assertRight(parsed);
    expect(parsed.right).toEqual({
      modalEnv: "main",
      modalRegions: [...DEFAULT_MODAL_REGIONS],
    });
    expect(parsed.right.modalRegions).toEqual(["us"]);
  });

  it("accepts an empty region list to opt out of pinning", () => {
    const parsed = parseSchema(ModalSandboxOptionsSchema, {
      modalRegions: [],
    });
    assertRight(parsed);
    expect(parsed.right.modalRegions).toEqual([]);
  });

  it("rejects blank region identifiers", () => {
    const parsed = parseSchema(ModalSandboxOptionsSchema, {
      modalRegions: [""],
    });
    assertLeft(parsed);
  });
});
