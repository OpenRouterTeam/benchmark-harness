import { describe, expect, it } from "bun:test";

import { GENERATION_ID_LINE_PREFIX } from "./generation-proxy";
import type { OriRunScriptOptions } from "./harness";
import { ORI_HARNESSES } from "./harness";

const FIRST_ID = "gen-1788473388-R386NZLdIly5wVTk0l5f";
const SECOND_ID = "gen-1788473571-DnfmpihRmJf33b9GDEF1";

const RUN_OPTIONS: OriRunScriptOptions = {
  instructionPath: "/instruction.md",
  logPath: "/logs/agent/agent.txt",
  reasoningEffort: "high",
  hasSystemPrompt: false,
  hasAppendSystemPrompt: true,
  hasAllowedTools: false,
  hasDisallowedTools: true,
  isolateAgentConfig: true,
};

const CODEX_STREAM = [
  "Reading additional input from stdin...",
  JSON.stringify({
    type: "thread.started",
    thread_id: "01a06952-2c38-7a13-9c4b-6c0f69fee5e0",
  }),
  JSON.stringify({ type: "turn.started" }),
  JSON.stringify({
    type: "item.completed",
    item: {
      id: "item_0",
      type: "error",
      message: "Model metadata for `openai/gpt-5-mini` not found.",
    },
  }),
  `${GENERATION_ID_LINE_PREFIX}${FIRST_ID}`,
  JSON.stringify({
    type: "item.completed",
    item: {
      id: "item_1",
      type: "command_execution",
      command: "ls",
      status: "completed",
    },
  }),
  `${GENERATION_ID_LINE_PREFIX}${SECOND_ID}`,
  `${GENERATION_ID_LINE_PREFIX}${FIRST_ID}`,
  JSON.stringify({
    type: "item.completed",
    item: { id: "item_2", type: "agent_message", text: "The sky is blue." },
  }),
  JSON.stringify({
    type: "turn.completed",
    usage: {
      input_tokens: 7620,
      cached_input_tokens: 0,
      cache_write_input_tokens: 0,
      output_tokens: 136,
      reasoning_output_tokens: 64,
    },
  }),
].join("\n");

const OPENCODE_STREAM = [
  JSON.stringify({
    type: "step_start",
    sessionID: "ses_1",
    part: { id: "prt_0", type: "step-start" },
  }),
  `${GENERATION_ID_LINE_PREFIX}${FIRST_ID}`,
  JSON.stringify({
    type: "tool_use",
    sessionID: "ses_1",
    part: {
      id: "prt_1",
      type: "tool",
      tool: "read",
      state: { status: "completed" },
    },
  }),
  JSON.stringify({
    type: "text",
    sessionID: "ses_1",
    part: { id: "prt_2", type: "text", text: "LE CIEL EST BLEU." },
  }),
  JSON.stringify({
    type: "step_finish",
    sessionID: "ses_1",
    part: {
      id: "prt_3",
      type: "step-finish",
      reason: "stop",
      tokens: {
        total: 4886,
        input: 4642,
        output: 116,
        reasoning: 128,
        cache: { write: 10, read: 20 },
      },
      cost: 0.0016485,
    },
  }),
].join("\n");

describe("codex harness", () => {
  it("parses usage, tool calls, the final message and proxy generation ids", () => {
    const run = ORI_HARNESSES.codex.parseRun(CODEX_STREAM);
    expect(run.generationIds).toEqual([FIRST_ID, SECOND_ID]);
    expect(run.usage).toEqual({
      inputTokens: 7620,
      outputTokens: 136,
      totalTokens: 7756,
      reasoningTokens: 64,
      totalCost: 0,
    });
    expect(run.finalText).toBe("The sky is blue.");
    expect(run.assistantMessages).toEqual([
      { role: "assistant", content: "The sky is blue." },
    ]);
    expect(run.turns).toBe(1);
    expect(run.toolCalls).toBe(1);
    expect(run.isError).toBe(false);
    expect(run.responseItems).toHaveLength(6);
  });

  it("flags failed turns with the upstream message", () => {
    const run = ORI_HARNESSES.codex.parseRun(
      [
        JSON.stringify({ type: "turn.started" }),
        JSON.stringify({
          type: "turn.failed",
          error: { message: "unexpected status 401 Unauthorized" },
        }),
      ].join("\n")
    );
    expect(run.isError).toBe(true);
    expect(run.apiErrorStatus).toBe("unexpected status 401 Unauthorized");
    expect(run.usage).toBeUndefined();
    expect(run.generationIds).toEqual([]);
  });

  it("routes codex through the proxy with exec-level provider overrides", () => {
    const script = ORI_HARNESSES.codex.buildRunScript({
      ...RUN_OPTIONS,
      hasSystemPrompt: true,
      hasDisallowedTools: false,
    });
    expect(script).toContain('ori codex --model "$TB_MODEL"');
    expect(script).toContain("--reasoning-effort high --");
    expect(script).toContain("exec --json --ephemeral --skip-git-repo-check");
    expect(script).toContain('-m "$TB_MODEL"');
    expect(script).toContain("-c model_reasoning_effort=high");
    expect(script).toContain("-c model_provider=openrouter");
    expect(script).toContain(
      "-c \"model_providers.openrouter.base_url='$OR_GENERATION_PROXY_BASE_URL'\""
    );
    expect(script).toContain(
      "-c \"model_providers.openrouter.wire_api='responses'\""
    );
    expect(script).toContain(
      "-c \"model_providers.openrouter.env_http_headers.X-Session-Id='ORI_OPENROUTER_SESSION_ID'\""
    );
    expect(script).toContain(
      "-c \"model_providers.openrouter.auth.args=['-c','echo \\$OPENROUTER_API_KEY']\""
    );
    expect(script).toContain(
      "-c \"model_instructions_file='/tmp/agent-system-prompt.md'\""
    );
    expect(script).toContain("developer_instructions=");
    expect(script).toContain("--ignore-user-config");
    expect(script).toContain("--ignore-rules");
    expect(script).toContain("-c project_doc_max_bytes=0");
    expect(script).toContain('"$(cat /instruction.md)"');
    expect(script).toContain("tee -a /logs/agent/agent.txt");
    expect(script.indexOf("export OR_GENERATION_PROXY_BASE_URL")).toBeLessThan(
      script.indexOf("ori codex")
    );
  });

  it("rejects tool allow and deny lists", () => {
    const script = ORI_HARNESSES.codex.buildRunScript(RUN_OPTIONS);
    expect(script).toContain(
      "Codex does not support allowedTools or disallowedTools"
    );
    expect(script).toContain("exit 2");
  });
});

describe("opencode and kilo harnesses", () => {
  it("parses usage, cost, tool calls, the final message and proxy generation ids", () => {
    for (const harness of [ORI_HARNESSES.opencode, ORI_HARNESSES.kilo]) {
      const run = harness.parseRun(OPENCODE_STREAM);
      expect(run.generationIds).toEqual([FIRST_ID]);
      expect(run.usage).toEqual({
        inputTokens: 4672,
        outputTokens: 244,
        totalTokens: 4886,
        reasoningTokens: 128,
        totalCost: 0.0016485,
      });
      expect(run.finalText).toBe("LE CIEL EST BLEU.");
      expect(run.turns).toBe(1);
      expect(run.toolCalls).toBe(1);
      expect(run.isError).toBe(false);
    }
  });

  it("flags error events with the nested message", () => {
    const run = ORI_HARNESSES.opencode.parseRun(
      JSON.stringify({
        type: "error",
        error: { name: "ProviderAuthError", data: { message: "bad key" } },
      })
    );
    expect(run.isError).toBe(true);
    expect(run.apiErrorStatus).toBe("bad key");
  });

  it("builds the proxy config after the proxy is up and disables denied tools", () => {
    for (const binary of ["opencode", "kilo"] as const) {
      const script = ORI_HARNESSES[binary].buildRunScript(RUN_OPTIONS);
      expect(script).toContain(
        `export ${binary.toUpperCase()}_CONFIG_CONTENT=`
      );
      expect(script).toContain(
        "baseURL: process.env.OR_GENERATION_PROXY_BASE_URL"
      );
      expect(script).toContain(
        '"X-Session-Id": "{env:ORI_OPENROUTER_SESSION_ID}"'
      );
      expect(script).toContain('instructions: ["/tmp/agent-append-prompt.md"]');
      expect(script).toContain("process.env.TB_DISALLOWED_TOOLS");
      expect(script).toContain(`ori ${binary} --model "$TB_MODEL"`);
      expect(script).toContain("--reasoning-effort high --");
      expect(script).toContain("run --format json --auto");
      expect(script).toContain("--pure");
      const prefix = binary.toUpperCase();
      expect(script).toContain(
        `export ${prefix}_DISABLE_PROJECT_CONFIG=1 ${prefix}_DISABLE_CLAUDE_CODE=1 ${prefix}_DISABLE_EXTERNAL_SKILLS=1`
      );
      expect(
        script.indexOf("export OR_GENERATION_PROXY_BASE_URL")
      ).toBeLessThan(script.indexOf("_CONFIG_CONTENT="));
    }
  });

  it("omits optional configuration and rejects unsupported options", () => {
    const plain = ORI_HARNESSES.kilo.buildRunScript({
      ...RUN_OPTIONS,
      hasAppendSystemPrompt: false,
      hasDisallowedTools: false,
      isolateAgentConfig: false,
    });
    expect(plain).not.toContain("instructions:");
    expect(plain).not.toContain("TB_DISALLOWED_TOOLS");
    expect(plain).not.toContain("--pure");
    expect(plain).not.toContain("_DISABLE_PROJECT_CONFIG");
    const rejected = ORI_HARNESSES.opencode.buildRunScript({
      ...RUN_OPTIONS,
      hasAllowedTools: true,
    });
    expect(rejected).toContain(
      "opencode does not support systemPrompt or allowedTools"
    );
    expect(rejected).toContain("exit 2");
  });
});
