import { describe, expect, it } from "bun:test";

import type { Effect } from "effect/Effect";
import { either, gen, provide, runPromise } from "effect/Effect";
import type { Layer } from "effect/Layer";

import type { SolverError } from "../../harness/core";
import { assertLeft, assertRight } from "../../internal/testing";
import type {
  CreateSessionInput,
  ExecResult,
  SandboxSessionFactory,
} from "../harbor/sandbox";
import { makeFakeSandboxLayer, SandboxSession } from "../harbor/sandbox";
import type { TerminalBench4SampleMeta } from "./dataset";
import {
  agentNetworkDeviation,
  artifactBundleCommand,
  createAgentSession,
  createVerifierSession,
  runCollectHooks,
  runVerifier,
  sandboxCollectHooks,
  transferArtifacts,
} from "./session";

const OK: ExecResult = { exitCode: 0, stdout: "", stderr: "" };

const META: TerminalBench4SampleMeta = {
  taskId: "fp8-rmsnorm-gemm",
  maxAgentTimeoutSec: 100,
  maxTestTimeoutSec: 50,
  category: "machine-learning",
  agentEnv: {
    cpus: 8,
    memoryMb: 16384,
    gpu: "H100",
    env: { HF_HOME: "/cache" },
    allowInternet: false,
  },
  verifierEnv: {
    cpus: 4,
    memoryMb: 8192,
    env: { TB_SEED: "1" },
    allowInternet: true,
  },
  artifacts: ["/app/out"],
  collect: [],
};

interface Recorded {
  readonly creates: CreateSessionInput[];
  readonly execs: {
    argv: readonly string[];
    env: Readonly<Record<string, string>>;
    timeoutMs: number | undefined;
  }[];
  readonly uploads: { localPath: string; remotePath: string }[];
  readonly downloads: { remotePath: string; localPath: string }[];
}

function makeFactory(
  recorded: Recorded,
  execHandler: (argv: readonly string[]) => ExecResult = () => OK
) {
  return makeFakeSandboxLayer({
    onCreate: (input) => {
      recorded.creates.push(input);
    },
    onUploadFile: (localPath, remotePath) => {
      recorded.uploads.push({ localPath, remotePath });
    },
    onDownloadFile: (remotePath, localPath) => {
      recorded.downloads.push({ remotePath, localPath });
    },
    execHandler: (argv, env, timeoutMs) => {
      recorded.execs.push({ argv, env, timeoutMs });
      return execHandler(argv);
    },
  });
}

function emptyRecorded(): Recorded {
  return { creates: [], execs: [], uploads: [], downloads: [] };
}

function withFactory<A>(
  layer: Layer<SandboxSession>,
  body: (factory: SandboxSessionFactory) => Effect<A, SolverError>
): Promise<A> {
  return runPromise(
    gen(function* () {
      const factory = yield* SandboxSession;
      return yield* body(factory);
    }).pipe(provide(layer))
  );
}

describe("terminal-bench-4 sandbox creation", () => {
  it("passes gpu, env and resources into the agent sandbox and forces internet on", async () => {
    const recorded = emptyRecorded();
    await withFactory(makeFactory(recorded), (sessionFactory) =>
      createAgentSession({
        sessionFactory,
        meta: META,
        tasksDir: "/tasks",
        imageTag: "repo/task:abc",
        imageBuildSteps: ["RUN echo hi"],
      })
    );
    expect(recorded.creates).toHaveLength(1);
    const input = recorded.creates[0];
    expect(input?.imageTag).toBe("repo/task:abc");
    expect(input?.imageBuildSteps).toEqual(["RUN echo hi"]);
    expect(input?.cpus).toBe(8);
    expect(input?.memoryMb).toBe(16384);
    expect(input?.gpu).toBe("H100");
    expect(input?.env).toEqual({ HF_HOME: "/cache" });
    expect(input?.allowInternet).toBe(true);
    expect(input?.timeoutSec).toBe(400);
    expect(input?.uploads).toEqual([
      {
        localPath: "/tasks/fp8-rmsnorm-gemm/instruction.md",
        remotePath: "/instruction.md",
        kind: "file",
      },
    ]);
  });

  it("creates the verifier sandbox from the verifier resources without a gpu", async () => {
    const recorded = emptyRecorded();
    await withFactory(makeFactory(recorded), (sessionFactory) =>
      createVerifierSession({
        sessionFactory,
        meta: META,
        imageTag: "repo/task-verifier:abc",
      })
    );
    const input = recorded.creates[0];
    expect(input?.imageTag).toBe("repo/task-verifier:abc");
    expect(input?.cpus).toBe(4);
    expect(input?.gpu).toBeUndefined();
    expect(input?.env).toEqual({ TB_SEED: "1" });
    expect(input?.timeoutSec).toBe(350);
    expect(input?.uploads).toEqual([]);
  });

  it("records a deviation only when the task disallows internet", () => {
    expect(agentNetworkDeviation(META)).toEqual({
      agentNetworkForced: true,
      taskAllowInternet: false,
    });
    expect(
      agentNetworkDeviation({
        ...META,
        agentEnv: { ...META.agentEnv, allowInternet: true },
      })
    ).toEqual({});
  });
});

describe("terminal-bench-4 collect hooks", () => {
  it("accepts hooks for the main service", async () => {
    const result = await runPromise(
      either(
        sandboxCollectHooks([
          { command: "a" },
          { command: "b", service: "main" },
        ])
      )
    );
    assertRight(result);
    expect(result.right).toHaveLength(2);
  });

  it("rejects hooks that target a compose sidecar", async () => {
    const result = await runPromise(
      either(sandboxCollectHooks([{ command: "a", service: "db" }]))
    );
    assertLeft(result);
    expect(result.left.message).toContain('"db"');
  });

  it("runs each hook through bash with its own timeout", async () => {
    const recorded = emptyRecorded();
    await withFactory(makeFactory(recorded), (sessionFactory) =>
      gen(function* () {
        const session = yield* sessionFactory.create({
          imageTag: "x",
          timeoutSec: 1,
          cpus: 1,
          memoryMb: 1,
          allowInternet: true,
          workdir: "/",
          keepAliveCommand: [],
          uploads: [],
        });
        yield* runCollectHooks(session, [
          { command: "echo one", timeout_sec: 5 },
          { command: "echo two" },
        ]);
        return session;
      })
    );
    expect(recorded.execs.map((e) => e.argv)).toEqual([
      ["bash", "-c", "echo one"],
      ["bash", "-c", "echo two"],
    ]);
    expect(recorded.execs.map((e) => e.timeoutMs)).toEqual([5000, 300_000]);
  });
});

describe("terminal-bench-4 artifact bundling", () => {
  it("always includes /logs/artifacts and dedupes declared sources", () => {
    const cmd = artifactBundleCommand([
      "/app/out",
      { source: "/app/out", exclude: ["*.tmp"] },
      { source: "/data", exclude: ["*.tmp", "cache/"] },
    ]);
    expect(cmd.match(/'\/logs\/artifacts'/g)).toHaveLength(2);
    expect(cmd.match(/'\/app\/out'/g)).toHaveLength(2);
    expect(cmd).toContain("--exclude='*.tmp' --exclude='cache/'");
    expect(cmd).toContain("tar -cf /tmp/tb4-artifacts.tar -P");
  });

  it("produces an empty tarball when nothing exists", () => {
    expect(artifactBundleCommand([])).toContain("-T /dev/null");
  });

  it("shell-quotes single quotes in paths", () => {
    expect(artifactBundleCommand(["/it's"])).toContain(String.raw`'/it'\''s'`);
  });

  it("bundles on the agent, moves the tarball, and extracts at absolute paths on the verifier", async () => {
    const recorded = emptyRecorded();
    await withFactory(makeFactory(recorded), (sessionFactory) =>
      gen(function* () {
        const create = (tag: string) =>
          sessionFactory.create({
            imageTag: tag,
            timeoutSec: 1,
            cpus: 1,
            memoryMb: 1,
            allowInternet: true,
            workdir: "/",
            keepAliveCommand: [],
            uploads: [],
          });
        const agent = yield* create("agent");
        const verifier = yield* create("verifier");
        yield* transferArtifacts({ agent, verifier, artifacts: ["/app/out"] });
        return agent;
      })
    );
    expect(recorded.downloads).toHaveLength(1);
    expect(recorded.downloads[0]?.remotePath).toBe("/tmp/tb4-artifacts.tar");
    expect(recorded.uploads).toHaveLength(1);
    expect(recorded.uploads[0]?.remotePath).toBe("/tmp/tb4-artifacts.tar");
    expect(recorded.uploads[0]?.localPath).toBe(
      recorded.downloads[0]?.localPath
    );
    const extract = recorded.execs.at(-1);
    expect(extract?.argv[2]).toBe(
      "tar -xf /tmp/tb4-artifacts.tar -P -C / && rm -f /tmp/tb4-artifacts.tar"
    );
  });
});

describe("terminal-bench-4 verifier", () => {
  async function runWith(
    execHandler: (argv: readonly string[]) => ExecResult
  ): Promise<{
    recorded: Recorded;
    result: { reward: number; output: string };
  }> {
    const recorded = emptyRecorded();
    const result = await withFactory(
      makeFactory(recorded, execHandler),
      (sessionFactory) =>
        gen(function* () {
          const verifier = yield* sessionFactory.create({
            imageTag: "v",
            timeoutSec: 1,
            cpus: 1,
            memoryMb: 1,
            allowInternet: true,
            workdir: "/",
            keepAliveCommand: [],
            uploads: [],
          });
          return yield* runVerifier(verifier, META);
        })
    );
    return { recorded, result };
  }

  it("runs /tests/test.sh with the task timeout and reads reward.txt", async () => {
    const { recorded, result } = await runWith((argv) =>
      argv[0] === "cat"
        ? { exitCode: 0, stdout: "1\n", stderr: "" }
        : { exitCode: 0, stdout: "PASS", stderr: "warn" }
    );
    expect(recorded.execs[0]?.argv).toEqual([
      "bash",
      "-c",
      "mkdir -p /logs/verifier && bash /tests/test.sh",
    ]);
    expect(recorded.execs[0]?.timeoutMs).toBe(55_000);
    expect(recorded.execs[1]?.argv).toEqual([
      "cat",
      "/logs/verifier/reward.txt",
    ]);
    expect(result).toEqual({ reward: 1, output: "PASS\nwarn" });
  });

  it("scores a missing or non-passing reward as zero", async () => {
    const missing = await runWith((argv) =>
      argv[0] === "cat" ? { exitCode: 1, stdout: "", stderr: "no file" } : OK
    );
    expect(missing.result.reward).toBe(0);
    const partial = await runWith((argv) =>
      argv[0] === "cat" ? { exitCode: 0, stdout: "0.5", stderr: "" } : OK
    );
    expect(partial.result.reward).toBe(0);
  });
});
