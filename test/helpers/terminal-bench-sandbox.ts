import type { Layer } from "effect/Layer";

import type {
  CreateSessionInput,
  ExecResult,
} from "../../src/benchmarks/harbor/sandbox";
import {
  makeFakeSandboxLayer,
  SandboxSession,
} from "../../src/benchmarks/harbor/sandbox";

export interface FakeTerminalBenchExecCall {
  readonly argv: readonly string[];
  readonly env: Readonly<Record<string, string>>;
  readonly timeoutMs: number | undefined;
}

export interface FakeTerminalBenchBehavior {
  readonly reward: number;
  readonly testOutput?: string;
  readonly agentEventStream?: string;
  readonly agentExitCode?: number;
  readonly execCalls?: FakeTerminalBenchExecCall[];
  readonly creates?: CreateSessionInput[];
}

const AGENT_COMMAND_MARKERS = ["pi --print", "pi ", "ori claude"] as const;

function isAgentCommand(joined: string): boolean {
  return AGENT_COMMAND_MARKERS.some((marker) => joined.includes(marker));
}

export function makeTerminalBenchFakeSandboxLayer(
  behavior: FakeTerminalBenchBehavior
): Layer<SandboxSession> {
  return makeFakeSandboxLayer({
    onCreate: (input) => {
      behavior.creates?.push(input);
    },
    execHandler: (argv, env, timeoutMs): ExecResult => {
      behavior.execCalls?.push({ argv: [...argv], env: { ...env }, timeoutMs });
      const joined = argv.join(" ");
      if (isAgentCommand(joined)) {
        return {
          stdout: behavior.agentEventStream ?? "",
          stderr: "",
          exitCode: behavior.agentExitCode ?? 0,
        };
      }
      if (joined.includes("cat /logs/verifier/reward")) {
        return { stdout: String(behavior.reward), stderr: "", exitCode: 0 };
      }
      return { stdout: behavior.testOutput ?? "", stderr: "", exitCode: 0 };
    },
  });
}

export { SandboxSession };
