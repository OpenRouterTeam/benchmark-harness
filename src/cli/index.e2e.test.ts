import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const cliPath = fileURLToPath(new URL("index.ts", import.meta.url));

interface FailureCase {
  readonly name: string;
  readonly args: readonly string[];
  readonly message: string;
  readonly env?: Readonly<Record<string, string | undefined>>;
}

const failureCases: readonly FailureCase[] = [
  {
    name: "image detail before cost tier and reasoning effort",
    args: [
      "--image-detail",
      "ultra",
      "--cost-tier",
      "bogus",
      "--reasoning-effort",
      "nope",
    ],
    message: "--image-detail must be one of:",
  },
  {
    name: "cost tier before reasoning effort",
    args: ["--cost-tier", "bogus", "--reasoning-effort", "nope"],
    message: "--cost-tier must be one of:",
  },
  {
    name: "invalid reasoning effort",
    args: ["--reasoning-effort", "nope"],
    message: "--reasoning-effort must be one of:",
  },
  {
    name: "adaptive effort on an unsupported model",
    args: ["--model", "openai/gpt-5", "--reasoning-effort", "auto"],
    message: "--reasoning-effort auto is only supported for:",
  },
  {
    name: "unknown benchmark before missing API key",
    args: ["--benchmark", "not_a_benchmark"],
    env: { OPENROUTER_API_KEY: undefined },
    message: 'Unknown benchmark "not_a_benchmark".',
  },
  {
    name: "missing API key before unsafe session ID",
    args: ["--model", "openai/gpt-5"],
    env: { OPENROUTER_API_KEY: undefined, BENCH_CHILD_WORKFLOW_ID: "trial\n" },
    message:
      "Set OPENROUTER_API_KEY (or BENCHMARKING_OPENROUTER_API_KEY) in the environment.",
  },
  {
    name: "unsafe session ID before malformed JSON",
    args: ["--model", "openai/gpt-5", "--solver-config", "{bad"],
    env: { BENCH_CHILD_WORKFLOW_ID: "trial\n" },
    message:
      "BENCH_CHILD_WORKFLOW_ID contains a control character, which is not a valid x-session-id header value",
  },
  {
    name: "malformed solver config JSON",
    args: ["--model", "openai/gpt-5", "--solver-config", "{bad"],
    message: "--solver-config is not valid JSON:",
  },
  {
    name: "missing model before unknown solver options",
    args: ["--solver-config", '{"bogus":1}'],
    message: "gpqa_diamond requires --model",
  },
  {
    name: "unknown solver options",
    args: ["--model", "openai/gpt-5", "--solver-config", '{"bogus":1}'],
    message: "Unknown gpqa_diamond solver-config option(s): bogus",
  },
  {
    name: "invalid benchmark config",
    args: [
      "--model",
      "openai/gpt-5",
      "--solver-config",
      '{"maxTokens":"invalid"}',
    ],
    message: "Invalid gpqa_diamond config:",
  },
];

describe("CLI validation boundary", () => {
  it.each(failureCases)(
    "reports $name without an uncaught exception",
    ({ args, message, env }) => {
      const result = spawnSync(process.execPath, [cliPath, ...args], {
        env: {
          OPENROUTER_API_KEY: "test-cli-key",
          BENCH_CHILD_WORKFLOW_ID: "cli-validation-test",
          ...env,
        },
        stdio: "pipe",
      });
      const stderr = result.stderr.toString();
      expect(result.status).toBe(1);
      expect(result.stdout.toString()).toBe("");
      expect(stderr).toStartWith(`error: ${message}`);
      expect(stderr.match(/^error:/gm)).toHaveLength(1);
      expect(stderr).not.toMatch(/^\s+at |^\d+\s*\|/m);
    }
  );
});
