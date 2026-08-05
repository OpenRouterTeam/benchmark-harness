import { Tag } from "effect/Context";
import type { Effect } from "effect/Effect";
import {
  catchAll,
  gen,
  succeed,
  tryPromise,
  void as effectVoid,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed as layerSucceed } from "effect/Layer";

import { SolverError } from "../../harness/core";

export interface ExecResult {
  readonly stdout: string;
  readonly stderr: string;
  readonly exitCode: number;
}

export interface TestResult {
  readonly reward: number;
  readonly output: string;
}

export interface SandboxSessionInstance {
  readonly sandboxId: string;
  readonly exec: (
    argv: string[],
    env: Readonly<Record<string, string>>,
    timeoutMs: number
  ) => Effect<ExecResult, SolverError>;
  readonly runTests: () => Effect<TestResult, SolverError>;
  readonly destroy: () => Effect<void, SolverError>;
}

export interface CreateSessionInput {
  readonly imageTag: string;
  readonly maxAgentTimeoutSec: number;
  readonly maxTestTimeoutSec: number;
  readonly testDir: string;
  readonly testScript: string;
  readonly instructionPath: string;
  readonly imageBuildSteps?: readonly string[];
}

export class SandboxSession extends Tag(
  "@openrouter/bench-harness/benchmarks/terminal-bench/sandbox/SandboxSession"
)<
  SandboxSession,
  {
    readonly create: (
      input: CreateSessionInput
    ) => Effect<SandboxSessionInstance, SolverError>;
  }
>() {}

export type SandboxSessionFactory = {
  readonly create: (
    input: CreateSessionInput
  ) => Effect<SandboxSessionInstance, SolverError>;
};

export function toSolverError(context: string, cause: unknown): SolverError {
  return new SolverError({ message: `${context}: ${String(cause)}` });
}

export type SandboxExec = (
  argv: string[],
  env: Readonly<Record<string, string>>,
  timeoutMs: number
) => Effect<ExecResult, SolverError>;

export interface MakeSessionInstanceInput {
  readonly sandboxId: string;
  readonly exec: SandboxExec;
  readonly maxTestTimeoutSec: number;
  readonly terminate: () => Promise<unknown>;
}

const REMOTE_REWARD_PATH = "/logs/verifier/reward.txt" as const;

const REMOTE_TEST_SCRIPT = "/tests/test.sh" as const;

export function makeSessionInstance(
  input: MakeSessionInstanceInput
): SandboxSessionInstance {
  return {
    sandboxId: input.sandboxId,
    exec: input.exec,
    runTests: () =>
      gen(function* runTests() {
        const timeoutMs = Math.round(input.maxTestTimeoutSec * 1000) + 5000;
        const r = yield* input.exec(
          [
            "bash",
            "-c",
            `mkdir -p /logs/verifier && bash ${REMOTE_TEST_SCRIPT}`,
          ],
          {},
          timeoutMs
        );
        const rewardRead = yield* input.exec(
          ["cat", REMOTE_REWARD_PATH],
          {},
          10000
        );
        return {
          reward: rewardRead.stdout.trim() === "1" ? 1 : 0,
          output: `${r.stdout}\n${r.stderr}`.trim(),
        };
      }),
    destroy: () =>
      tryPromise({
        try: input.terminate,
        catch: (e) =>
          toSolverError(`Failed to terminate ${input.sandboxId}`, e),
      }).pipe(catchAll(() => effectVoid)),
  };
}

export const REMOTE_TEST_DIR = "/tests" as const;

export const REMOTE_INSTRUCTION = "/instruction.md" as const;

export const CONTAINER_WORKDIR = "/app" as const;

export interface FakeSandboxBehavior {
  readonly reward: number;
  readonly testOutput?: string;
  readonly agentEventStream?: string;
  readonly agentExitCode?: number;
  readonly execCalls?: FakeSandboxExecCall[];
}

export interface FakeSandboxExecCall {
  readonly argv: readonly string[];
  readonly env: Readonly<Record<string, string>>;
  readonly timeoutMs: number;
}

export function makeFakeSandboxLayer(
  behavior: FakeSandboxBehavior
): Layer<SandboxSession> {
  const create = (
    _input: CreateSessionInput
  ): Effect<SandboxSessionInstance, SolverError> =>
    succeed({
      sandboxId: "fake-sandbox",
      exec: (argv, env, timeoutMs) => {
        behavior.execCalls?.push({
          argv: [...argv],
          env: { ...env },
          timeoutMs,
        });
        const joined = argv.join(" ");
        if (joined.includes("pi ") || joined.includes("pi --print")) {
          return succeed({
            stdout: behavior.agentEventStream ?? "",
            stderr: "",
            exitCode: behavior.agentExitCode ?? 0,
          });
        }
        if (joined.includes("cat /logs/verifier/reward")) {
          return succeed({
            stdout: String(behavior.reward),
            stderr: "",
            exitCode: 0,
          });
        }
        return succeed({
          stdout: behavior.testOutput ?? "",
          stderr: "",
          exitCode: 0,
        });
      },
      runTests: () =>
        succeed({ reward: behavior.reward, output: behavior.testOutput ?? "" }),
      destroy: () => effectVoid,
    });
  return layerSucceed(SandboxSession, { create });
}
