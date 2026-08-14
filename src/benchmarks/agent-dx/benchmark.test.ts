import { describe, expect, it } from "bun:test";

import { assertRight } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { datasetSizeById } from "../../runner/run-by-id";
import type { AgentDxConfig } from "../benchmark-config";
import { AgentDxConfigSchema } from "../benchmark-config";
import { invalidSandboxKeyPairing } from "./benchmark";

function agentDxConfig(overrides: Record<string, unknown> = {}): AgentDxConfig {
  const parsed = parseSchema(AgentDxConfigSchema, {
    benchmarkId: "agent_dx",
    model: "test/model",
    ...overrides,
  });
  assertRight(parsed);
  return parsed.right;
}

describe("agent_dx dataset sizing", () => {
  it("sizes the default benchmark suite without a config", async () => {
    const size = await datasetSizeById("agent_dx");
    assertRight(size);
    expect(size.right).toBe(12);
  });

  it("sizes the configured regression suite, not the default corpus", async () => {
    const size = await datasetSizeById(
      "agent_dx",
      agentDxConfig({ suite: "regression" })
    );
    assertRight(size);
    expect(size.right).toBe(1);
  });

  it("sizes a configured task subset", async () => {
    const size = await datasetSizeById(
      "agent_dx",
      agentDxConfig({ taskSubset: ["basic-completion", "model-discovery"] })
    );
    assertRight(size);
    expect(size.right).toBe(2);
  });
});

describe("agent_dx sandbox-key pairing", () => {
  it("rejects absent-key mode outside the discoverability suite", () => {
    expect(
      invalidSandboxKeyPairing(agentDxConfig({ sandboxKey: "absent" }))
    ).toContain('requires suite "discoverability"');
    expect(
      invalidSandboxKeyPairing(
        agentDxConfig({ sandboxKey: "absent", suite: "regression" })
      )
    ).toContain('requires suite "discoverability"');
  });

  it("accepts absent-key discoverability and provided-key runs of any suite", () => {
    expect(
      invalidSandboxKeyPairing(
        agentDxConfig({ sandboxKey: "absent", suite: "discoverability" })
      )
    ).toBeUndefined();
    expect(invalidSandboxKeyPairing(agentDxConfig({}))).toBeUndefined();
    expect(
      invalidSandboxKeyPairing(agentDxConfig({ suite: "regression" }))
    ).toBeUndefined();
  });

  it("rejects absent-key mode when a task subset selects non-discoverability tasks", () => {
    expect(
      invalidSandboxKeyPairing(
        agentDxConfig({
          sandboxKey: "absent",
          suite: "discoverability",
          taskSubset: ["basic-completion"],
        })
      )
    ).toContain("every taskSubset task");
  });

  it("accepts absent-key mode with a discoverability task subset", () => {
    expect(
      invalidSandboxKeyPairing(
        agentDxConfig({
          sandboxKey: "absent",
          suite: "discoverability",
          taskSubset: ["which-model-question", "no-such-task"],
        })
      )
    ).toBeUndefined();
  });
});
