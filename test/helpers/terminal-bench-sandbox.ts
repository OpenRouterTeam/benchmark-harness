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
  readonly uploadedDirs?: { localDir: string; remoteDir: string }[];
  readonly failAgentExec?: boolean;
  readonly recoveredLog?: string;
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
    onUploadDir: (localDir, remoteDir) => {
      behavior.uploadedDirs?.push({ localDir, remoteDir });
    },
    execHandler: (argv, env, timeoutMs): ExecResult => {
      behavior.execCalls?.push({ argv: [...argv], env: { ...env }, timeoutMs });
      const joined = argv.join(" ");
      if (joined.startsWith("cat /logs/agent/")) {
        return {
          stdout: behavior.recoveredLog ?? "",
          stderr: "",
          exitCode: 0,
        };
      }
      if (isAgentCommand(joined)) {
        if (behavior.failAgentExec === true) {
          throw new Error("Deadline exceeded while streaming stdio for exec");
        }
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
