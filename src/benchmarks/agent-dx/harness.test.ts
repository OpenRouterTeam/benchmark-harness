import { describe, expect, it } from "bun:test";

import {
  buildClaudeCodeImageSteps,
  buildClaudeCodeRunScript,
  buildOpencodeImageSteps,
  buildOpencodeRunScript,
  buildSkillsInstallSteps,
  parseClaudeCodeUsage,
  parseOpencodeUsage,
} from "./harness";

const SOURCES = {
  skillsSource: "https://github.com/OpenRouterTeam/skills",
  docsSource: "https://openrouter.ai/docs/llms-full.txt",
} as const;

function decodeAgentsMd(script: string): string {
  const match = script.match(
    /printf '%s' '([A-Za-z0-9+/=]+)' \| base64 -d > \/app\/AGENTS\.md/
  );
  return match?.[1] === undefined
    ? ""
    : Buffer.from(match[1], "base64").toString("utf8");
}

function decodeOpencodeJson(script: string): string {
  const match = script.match(
    /printf '%s' '([A-Za-z0-9+/=]+)' \| base64 -d > \/app\/opencode\.json/
  );
  return match?.[1] === undefined
    ? ""
    : Buffer.from(match[1], "base64").toString("utf8");
}

describe("buildOpencodeImageSteps", () => {
  it("omits skill install steps for the baseline and mcp profiles", () => {
    const baselineSteps = buildOpencodeImageSteps({
      opencodePackage: "opencode-ai@latest",
      profile: "baseline",
      skillsSource: "ignored",
      docsSource: "ignored",
    });
    const mcpSteps = buildOpencodeImageSteps({
      opencodePackage: "opencode-ai@latest",
      profile: "mcp",
      skillsSource: "ignored",
      docsSource: "ignored",
    });
    expect(baselineSteps.join("\n")).not.toContain("openrouter-skills");
    expect(mcpSteps.join("\n")).not.toContain("openrouter-skills");
    expect(baselineSteps.join("\n")).not.toContain("openrouter-docs");
  });

  it("fetches the pinned docs snapshot for the docs profile", () => {
    const steps = buildOpencodeImageSteps({
      opencodePackage: "opencode-ai@latest",
      profile: "docs",
      skillsSource: "ignored",
      docsSource: "https://openrouter.ai/docs/llms-full.txt",
    });
    expect(steps.join("\n")).toContain(
      "curl -fsSL https://openrouter.ai/docs/llms-full.txt -o /opt/openrouter-docs/llms-full.txt"
    );
  });

  it("rejects an opencode package with shell metacharacters at the build-command boundary", () => {
    expect(() =>
      buildOpencodeImageSteps({
        opencodePackage: "opencode-ai@latest; curl evil",
        profile: "baseline",
        skillsSource: "ignored",
        docsSource: "ignored",
      })
    ).toThrow(/invalid opencodePackage/);
  });

  it("rejects a docs source with shell metacharacters at the build-command boundary", () => {
    expect(() =>
      buildOpencodeImageSteps({
        opencodePackage: "opencode-ai@latest",
        profile: "docs",
        skillsSource: "ignored",
        docsSource: "https://x.test/docs.txt; curl evil",
      })
    ).toThrow(/invalid docsSource/);
  });
});

describe("buildSkillsInstallSteps", () => {
  it("clones shallow when no ref is pinned", () => {
    const steps = buildSkillsInstallSteps(
      "https://github.com/OpenRouterTeam/skills"
    );
    expect(steps[0]).toBe(
      "RUN git clone --depth 1 https://github.com/OpenRouterTeam/skills /opt/openrouter-skills"
    );
  });

  it("checks out a pinned ref when the source has a #ref suffix", () => {
    const steps = buildSkillsInstallSteps(
      "https://github.com/OpenRouterTeam/skills#abc123"
    );
    expect(steps[0]).toBe(
      "RUN git clone https://github.com/OpenRouterTeam/skills /opt/openrouter-skills && git -C /opt/openrouter-skills checkout abc123"
    );
  });

  it("rejects a skills source with shell metacharacters at the build-command boundary", () => {
    expect(() =>
      buildSkillsInstallSteps("https://x.test/repo; rm -rf /")
    ).toThrow(/invalid skillsSource/);
  });
});

describe("buildOpencodeRunScript", () => {
  it("omits MCP config and docs pointer for the baseline profile", () => {
    const script = buildOpencodeRunScript({ profile: "baseline" });
    expect(script).toContain("opencode run --format json");
    expect(script).not.toContain("/app/opencode.json");
    expect(script).not.toContain("/app/AGENTS.md");
  });

  it("points the agent at the pinned docs snapshot for the docs profile", () => {
    const script = buildOpencodeRunScript({ profile: "docs" });
    expect(script).toContain("/app/AGENTS.md");
    expect(decodeAgentsMd(script)).toContain(
      "/opt/openrouter-docs/llms-full.txt"
    );
    expect(script).not.toContain("/app/opencode.json");
  });

  it("writes a docs addendum as base64 and references it first in AGENTS.md", () => {
    const addendum =
      "# Model selection\nAlways query /api/v1/models; don't hardcode '; rm -rf /";
    const script = buildOpencodeRunScript({
      profile: "docs",
      docsAddendum: addendum,
    });
    expect(script).toContain("/opt/openrouter-docs/addendum.md");
    expect(decodeAgentsMd(script)).toContain(
      "Start with the latest OpenRouter guidance"
    );
    expect(script).toContain(Buffer.from(addendum, "utf8").toString("base64"));
    expect(script).not.toContain("rm -rf");
  });

  it("ignores the docs addendum outside the docs profile", () => {
    const script = buildOpencodeRunScript({
      profile: "baseline",
      docsAddendum: "addendum text",
    });
    expect(script).not.toContain("/opt/openrouter-docs/addendum.md");
  });

  it("writes the OpenRouter MCP config for the mcp profile", () => {
    const script = buildOpencodeRunScript({ profile: "mcp" });
    expect(script).toContain("opencode.json");
    expect(decodeOpencodeJson(script)).toContain(
      "https://mcp.openrouter.ai/mcp"
    );
    expect(decodeOpencodeJson(script)).toContain(
      "Bearer {env:OPENROUTER_API_KEY}"
    );
    expect(script).not.toContain("/app/AGENTS.md");
  });

  it("writes the mcp addendum as base64 into AGENTS.md for the mcp profile", () => {
    const addendum =
      "# OpenRouter MCP\nUse list-models when selecting a model; don't '; rm -rf /";
    const script = buildOpencodeRunScript({
      profile: "mcp",
      mcpAddendum: addendum,
    });
    expect(script).toContain("/app/AGENTS.md");
    expect(script).toContain(Buffer.from(addendum, "utf8").toString("base64"));
    expect(script).not.toContain("rm -rf");
  });

  it("ignores the mcp addendum outside the mcp profile", () => {
    const script = buildOpencodeRunScript({
      profile: "baseline",
      mcpAddendum: "addendum text",
    });
    expect(script).not.toContain("/app/AGENTS.md");
  });

  it("wires the MCP server but no docs pointer for the agents profile", () => {
    const script = buildOpencodeRunScript({ profile: "agents" });
    expect(decodeOpencodeJson(script)).toContain(
      "https://mcp.openrouter.ai/mcp"
    );
    expect(script).not.toContain("/app/AGENTS.md");
  });

  it("routes the opencode provider key through ADX_HARNESS_KEY in absent key mode", () => {
    const script = buildOpencodeRunScript({
      profile: "baseline",
      sandboxKey: "absent",
    });
    expect(script).toContain("/root/.config/opencode/opencode.json");
    const b64Match =
      /printf '%s' '([^']+)' \| base64 -d > \/root\/\.config\/opencode\/opencode\.json/.exec(
        script
      );
    const decoded = Buffer.from(b64Match?.[1] ?? "", "base64").toString("utf8");
    expect(decoded).toContain("{env:ADX_HARNESS_KEY}");
    expect(
      buildOpencodeRunScript({ profile: "baseline", sandboxKey: "provided" })
    ).not.toContain("/root/.config/opencode/opencode.json");
  });

  it("carries the provider key in the project-level MCP config in absent key mode", () => {
    const absentConfig = decodeOpencodeJson(
      buildOpencodeRunScript({ profile: "mcp", sandboxKey: "absent" })
    );
    expect(absentConfig).toContain('"provider"');
    expect(absentConfig).toContain("{env:ADX_HARNESS_KEY}");
    const providedConfig = decodeOpencodeJson(
      buildOpencodeRunScript({ profile: "mcp", sandboxKey: "provided" })
    );
    expect(providedConfig).not.toContain('"provider"');
  });
});

describe("parseOpencodeUsage", () => {
  const stepFinishEvent = JSON.stringify({
    type: "step_finish",
    part: {
      reason: "stop",
      tokens: {
        total: 8275,
        input: 538,
        output: 174,
        reasoning: 1,
        cache: { write: 0, read: 7562 },
      },
      cost: 0.0015238,
    },
  });

  it("sums tokens and cost across step_finish events", () => {
    const usage = parseOpencodeUsage(`${stepFinishEvent}\n${stepFinishEvent}`);
    expect(usage?.inputTokens).toBe(2 * (538 + 7562));
    expect(usage?.outputTokens).toBe(2 * 174);
    expect(usage?.reasoningTokens).toBe(2);
    expect(usage?.totalTokens).toBe(2 * 8275);
    expect(usage?.totalCost).toBeCloseTo(2 * 0.0015238, 7);
  });

  it("returns undefined when no usage events are present", () => {
    expect(parseOpencodeUsage('not json\n{"type":"text"}')).toBeUndefined();
  });
});

describe("buildClaudeCodeImageSteps", () => {
  it("installs the CLI only for baseline", () => {
    const steps = buildClaudeCodeImageSteps({
      profile: "baseline",
      ...SOURCES,
    });
    expect(
      steps.some((step) => step.includes("@anthropic-ai/claude-code"))
    ).toBe(true);
    expect(steps.some((step) => step.includes("git clone"))).toBe(false);
    expect(steps.some((step) => step.includes("/opt/openrouter-docs"))).toBe(
      false
    );
  });

  it("installs skills into the Claude Code user skills dir", () => {
    const steps = buildClaudeCodeImageSteps({ profile: "skills", ...SOURCES });
    expect(steps.some((step) => step.includes("git clone --depth 1"))).toBe(
      true
    );
    expect(steps.some((step) => step.includes("/root/.claude/skills"))).toBe(
      true
    );
  });

  it("honors a pinned skills ref", () => {
    const steps = buildClaudeCodeImageSteps({
      profile: "agents",
      skillsSource: "https://github.com/OpenRouterTeam/skills#candidate",
      docsSource: SOURCES.docsSource,
    });
    expect(
      steps.some(
        (step) => step.includes("checkout candidate") && !step.includes("#")
      )
    ).toBe(true);
  });

  it("rejects a skills source that escapes the build-command pattern", () => {
    expect(() =>
      buildClaudeCodeImageSteps({
        profile: "skills",
        skillsSource:
          "https://github.com/OpenRouterTeam/skills#main && rm -rf /",
        docsSource: SOURCES.docsSource,
      })
    ).toThrow("invalid skillsSource");
    expect(() =>
      buildClaudeCodeImageSteps({
        profile: "skills",
        skillsSource: "https://github.com/OpenRouterTeam/skills#-option",
        docsSource: SOURCES.docsSource,
      })
    ).toThrow("invalid skillsSource");
  });

  it("fetches the pinned docs snapshot for the docs profile", () => {
    const steps = buildClaudeCodeImageSteps({ profile: "docs", ...SOURCES });
    expect(
      steps.some((step) => step.includes("/opt/openrouter-docs/llms-full.txt"))
    ).toBe(true);
  });

  it("rejects a docs source that escapes the build-command pattern", () => {
    expect(() =>
      buildClaudeCodeImageSteps({
        profile: "docs",
        skillsSource: SOURCES.skillsSource,
        docsSource: "https://openrouter.ai/docs/llms-full.txt; rm -rf /",
      })
    ).toThrow("invalid docsSource");
    expect(() =>
      buildClaudeCodeImageSteps({
        profile: "docs",
        skillsSource: SOURCES.skillsSource,
        docsSource: "http://openrouter.ai/docs/llms-full.txt",
      })
    ).toThrow("invalid docsSource");
  });
});

describe("buildClaudeCodeRunScript", () => {
  it("wires Claude Code to OpenRouter per the integration guide", () => {
    const script = buildClaudeCodeRunScript({ profile: "baseline" });
    expect(script).toContain(
      // eslint-disable-next-line no-template-curly-in-string -- asserting shell param expansion in generated script
      'ANTHROPIC_BASE_URL="${ADX_OPENROUTER_ORIGIN:-https://openrouter.ai}/api"'
    );
    expect(script).toContain('ANTHROPIC_AUTH_TOKEN="$OPENROUTER_API_KEY"');
    expect(script).toContain('export ANTHROPIC_API_KEY=""');
    expect(script).toContain("export IS_SANDBOX=1");
    expect(script).toContain("export CLAUDE_CODE_DISABLE_BUNDLED_SKILLS=1");
    expect(script).toContain("--output-format stream-json");
    expect(script).not.toContain(".mcp.json");
  });

  it("registers the OpenRouter MCP server through a project .mcp.json", () => {
    const script = buildClaudeCodeRunScript({ profile: "mcp" });
    expect(script).toContain("/app/.mcp.json");
    expect(script).toContain("/app/.claude/settings.json");
    const b64Match =
      /printf '%s' '([^']+)' \| base64 -d > \/app\/\.mcp\.json/.exec(script);
    expect(b64Match).not.toBeNull();
    const decoded = Buffer.from(b64Match?.[1] ?? "", "base64").toString("utf8");
    expect(decoded).toContain("https://mcp.openrouter.ai/mcp");
    // eslint-disable-next-line no-template-curly-in-string -- asserting Claude Code ${VAR} expansion syntax
    expect(decoded).toContain("${OPENROUTER_API_KEY}");
  });

  it("authenticates through ADX_HARNESS_KEY in absent key mode", () => {
    const script = buildClaudeCodeRunScript({
      profile: "mcp",
      sandboxKey: "absent",
    });
    expect(script).toContain('ANTHROPIC_AUTH_TOKEN="$ADX_HARNESS_KEY"');
    expect(script).not.toContain("$OPENROUTER_API_KEY");
    const b64Match =
      /printf '%s' '([^']+)' \| base64 -d > \/app\/\.mcp\.json/.exec(script);
    const decoded = Buffer.from(b64Match?.[1] ?? "", "base64").toString("utf8");
    // eslint-disable-next-line no-template-curly-in-string -- asserting Claude Code ${VAR} expansion syntax
    expect(decoded).toContain("${ADX_HARNESS_KEY}");
  });

  it("surfaces the docs snapshot through CLAUDE.md for the docs profile", () => {
    const script = buildClaudeCodeRunScript({
      profile: "docs",
      docsAddendum: "candidate addendum",
    });
    expect(script).toContain("/app/CLAUDE.md");
    expect(script).toContain("/opt/openrouter-docs/addendum.md");
  });
});

describe("parseClaudeCodeUsage", () => {
  it("reads totals from the final result event, folding cache into input and ignoring the harness cost estimate", () => {
    const stream = [
      JSON.stringify({ type: "assistant", message: { content: [] } }),
      JSON.stringify({
        type: "result",
        subtype: "success",
        total_cost_usd: 0.42,
        usage: {
          input_tokens: 100,
          output_tokens: 50,
          cache_creation_input_tokens: 10,
          cache_read_input_tokens: 30,
        },
      }),
    ].join("\n");
    expect(parseClaudeCodeUsage(stream)).toEqual({
      inputTokens: 140,
      outputTokens: 50,
      totalTokens: 190,
      reasoningTokens: 0,
      totalCost: 0,
    });
  });

  it("returns undefined when no result event exists", () => {
    expect(
      parseClaudeCodeUsage('{"type":"assistant"}\nnot json')
    ).toBeUndefined();
  });
});
