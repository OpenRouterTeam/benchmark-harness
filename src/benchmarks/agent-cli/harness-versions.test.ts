import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { TerminalBenchOptionsSchema } from "../benchmark-config";
import {
  DEFAULT_OMP_PACKAGE,
  DEFAULT_PI_AGENT_PACKAGE,
  DEFAULT_PRIME_AGENT_PACKAGE,
  getOriHarness,
} from "./harness";
import {
  HARNESS_NODE_VERSION,
  HARNESS_VERSIONS,
  harnessPackage,
  resolveAgentPackage,
} from "./harness-versions";
import { agentImageBuildSteps } from "./runner";
import { DEFAULT_CLAUDE_PACKAGE, ORI_AGENTS } from "./schema";

const AGENT_RUNTIME_DIR = join(
  import.meta.dir,
  "../../../scripts/agent-runtime"
);

function dockerfileArg(name: string): string | undefined {
  const dockerfile = readFileSync(
    join(AGENT_RUNTIME_DIR, "Dockerfile"),
    "utf8"
  );
  return new RegExp(`^ARG ${name}=(.+)$`, "m").exec(dockerfile)?.[1];
}

describe("harness version manifest", () => {
  test("default packages render from the manifest", () => {
    expect(DEFAULT_CLAUDE_PACKAGE).toBe("@anthropic-ai/claude-code@2.1.235");
    expect(DEFAULT_PI_AGENT_PACKAGE).toBe(
      "@earendil-works/pi-coding-agent@0.84.2"
    );
    expect(DEFAULT_OMP_PACKAGE).toBe("@oh-my-pi/pi-coding-agent@18.1.2");
    expect(DEFAULT_PRIME_AGENT_PACKAGE).toBe(
      "https://pub-728493de92a943e2a9b2d17b4719f318.r2.dev/releases/v0.9.1/prime-agent-0.9.1.tgz"
    );
    for (const agent of ORI_AGENTS) {
      expect(getOriHarness(agent).defaultPackage).toBe(harnessPackage(agent));
    }
  });

  test("agent-runtime archive pins match the manifest", () => {
    const pkg: unknown = JSON.parse(
      readFileSync(join(AGENT_RUNTIME_DIR, "package.json"), "utf8")
    );
    expect(pkg).toMatchObject({
      dependencies: {
        "@anthropic-ai/claude-code": HARNESS_VERSIONS.harnesses.claude.version,
        "@earendil-works/pi-coding-agent":
          HARNESS_VERSIONS.harnesses.pi.version,
      },
    });
    expect(dockerfileArg("NODE_VERSION")).toBe(HARNESS_NODE_VERSION);
    expect(dockerfileArg("CLAUDE_CODE_VERSION")).toBe(
      HARNESS_VERSIONS.harnesses.claude.version
    );
    expect(dockerfileArg("PI_CODING_AGENT_VERSION")).toBe(
      HARNESS_VERSIONS.harnesses.pi.version
    );
    expect(dockerfileArg("PRIME_AGENT_VERSION")).toBe(
      HARNESS_VERSIONS.harnesses["prime-agent"].version
    );
    expect(dockerfileArg("PRIME_AGENT_PACKAGE_URL")).toBe(
      DEFAULT_PRIME_AGENT_PACKAGE
    );
  });

  test("agentVersion swaps only the version", () => {
    expect(harnessPackage("pi", "1.5.0")).toBe(
      "@earendil-works/pi-coding-agent@1.5.0"
    );
    expect(harnessPackage("prime-agent", "0.10.0")).toBe(
      "https://pub-728493de92a943e2a9b2d17b4719f318.r2.dev/releases/v0.10.0/prime-agent-0.10.0.tgz"
    );
    expect(harnessPackage("claude", "latest")).toBe(
      "@anthropic-ai/claude-code@latest"
    );
  });

  test("rejects unsafe versions and ambiguous overrides", () => {
    expect(() => harnessPackage("pi", "1.0.0; rm -rf /")).toThrow(
      "invalid agentVersion"
    );
    expect(() => harnessPackage("pi", "../evil")).toThrow(
      "invalid agentVersion"
    );
    expect(() =>
      resolveAgentPackage("pi", { agentPackage: "pi@1", agentVersion: "1.5.0" })
    ).toThrow("not both");
    expect(resolveAgentPackage("omp", { agentPackage: "custom-omp@1" })).toBe(
      "custom-omp@1"
    );
    expect(resolveAgentPackage("omp", {})).toBe(DEFAULT_OMP_PACKAGE);
  });

  test("config schema validates agentVersion", () => {
    const base = { agentReasoningEffort: "medium" } as const;
    expect(
      TerminalBenchOptionsSchema.parse({ ...base, agentVersion: "2.1.318" })
        .agentVersion
    ).toBe("2.1.318");
    expect(
      TerminalBenchOptionsSchema.safeParse({ ...base, agentVersion: "1 && x" })
        .success
    ).toBe(false);
  });
});

describe("agentImageBuildSteps with agentVersion", () => {
  const opts = {
    model: "openai/gpt-5",
    apiKey: "test",
    agentReasoningEffort: "medium",
  } as const;

  test("a non-default version installs that version instead of the archive", () => {
    const steps = agentImageBuildSteps(getOriHarness("claude"), {
      ...opts,
      agentVersion: "2.1.318",
    }).join("\n");
    expect(steps).toContain(
      `npm install -g ${JSON.stringify("@anthropic-ai/claude-code@2.1.318")}`
    );
    expect(steps).not.toContain("agent-runtime");
  });

  test("the default version keeps the prebuilt archive", () => {
    const steps = agentImageBuildSteps(getOriHarness("claude"), {
      ...opts,
      agentVersion: HARNESS_VERSIONS.harnesses.claude.version,
    }).join("\n");
    expect(steps).toContain("agent-runtime");
  });

  test("omp installs the requested version with bun", () => {
    const steps = agentImageBuildSteps(getOriHarness("omp"), {
      ...opts,
      agentVersion: "18.16.0",
    }).join("\n");
    expect(steps).toContain(
      `bun install -g ${JSON.stringify("@oh-my-pi/pi-coding-agent@18.16.0")}`
    );
  });
});
