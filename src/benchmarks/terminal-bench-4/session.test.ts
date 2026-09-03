import { describe, expect, it } from "bun:test";
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

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
  agentUserDeviation,
  agentSandboxTimeoutSec,
  ARTIFACT_BUNDLE_TIMEOUT_MS,
  ARTIFACT_TRANSFER_TIMEOUT_MS,
  artifactBundleCommand,
  artifactExtractCommand,
  createAgentSession,
  DEFAULT_COLLECT_TIMEOUT_SEC,
  SANDBOX_TIMEOUT_MARGIN_SEC,
  verifierSandboxTimeoutSec,
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
    expect(input?.imageKind).toBe("modal-image-id");
    expect(input?.imageBuildSteps).toEqual(["RUN echo hi"]);
    expect(input?.cpus).toBe(8);
    expect(input?.memoryMb).toBe(16384);
    expect(input?.gpu).toBe("H100");
    expect(input?.env).toEqual({ HF_HOME: "/cache" });
    expect(input?.allowInternet).toBe(true);
    expect(input?.timeoutSec).toBe(agentSandboxTimeoutSec(META));
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
    expect(input?.imageKind).toBe("modal-image-id");
    expect(input?.cpus).toBe(4);
    expect(input?.gpu).toBeUndefined();
    expect(input?.env).toEqual({ TB_SEED: "1" });
    expect(input?.timeoutSec).toBe(verifierSandboxTimeoutSec(META));
    expect(input?.uploads).toEqual([]);
  });

  it("keeps the agent sandbox alive through collect hooks and artifact transfer", () => {
    const meta: TerminalBench4SampleMeta = {
      ...META,
      maxAgentTimeoutSec: 28_800,
      collect: [
        { command: "a", timeout_sec: 120 },
        { command: "b" },
        { command: "c", timeout_sec: 0.5 },
      ],
    };
    const postProcessingSec =
      120 +
      DEFAULT_COLLECT_TIMEOUT_SEC +
      0.5 +
      (2 * ARTIFACT_BUNDLE_TIMEOUT_MS + ARTIFACT_TRANSFER_TIMEOUT_MS) / 1000;
    const lifetime = agentSandboxTimeoutSec(meta);
    expect(lifetime).toBeGreaterThanOrEqual(
      meta.maxAgentTimeoutSec + postProcessingSec
    );
    expect(lifetime).toBe(
      Math.ceil(
        meta.maxAgentTimeoutSec + postProcessingSec + SANDBOX_TIMEOUT_MARGIN_SEC
      )
    );
    expect(lifetime - agentSandboxTimeoutSec(META)).toBe(
      Math.ceil(28_700 + 120 + DEFAULT_COLLECT_TIMEOUT_SEC + 0.5)
    );
  });

  it("keeps the verifier sandbox alive through artifact extraction and the test run", () => {
    expect(verifierSandboxTimeoutSec(META)).toBeGreaterThanOrEqual(
      META.maxTestTimeoutSec +
        (2 * ARTIFACT_BUNDLE_TIMEOUT_MS + ARTIFACT_TRANSFER_TIMEOUT_MS) / 1000
    );
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

  it("records a deviation only when the task image declares a non-root USER", () => {
    expect(agentUserDeviation(META)).toEqual({});
    expect(agentUserDeviation({ ...META, imageUser: "nobody" })).toEqual({
      agentRunsAsRoot: true,
      taskImageUser: "nobody",
    });
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
    expect(extract?.argv[2]).toBe(artifactExtractCommand(["/app/out"]));
    expect(extract?.argv[2]).toContain(
      "tar -xf /tmp/tb4-artifacts.tar -P -C / && rm -f /tmp/tb4-artifacts.tar"
    );
  });

  it("clears declared directory destinations so files deleted by the agent do not survive", () => {
    const root = mkdtempSync(join(tmpdir(), "tb4-extract-"));
    try {
      const agentDir = join(root, "agent", "pkg");
      const verifierDir = join(root, "verifier", "pkg");
      mkdirSync(agentDir, { recursive: true });
      mkdirSync(verifierDir, { recursive: true });
      writeFileSync(join(agentDir, "kept.py"), "agent");
      writeFileSync(join(agentDir, "skipped.pyc"), "agent");
      writeFileSync(join(verifierDir, "kept.py"), "image");
      writeFileSync(join(verifierDir, "deleted.py"), "image");
      writeFileSync(join(verifierDir, "skipped.pyc"), "image");
      writeFileSync(join(root, "verifier", "untouched.txt"), "image");
      const bundle = join(root, "bundle.tar");
      const artifacts = [{ source: verifierDir, exclude: ["*.pyc"] }];
      const pack = spawnSync(
        "bash",
        [
          "-c",
          `tar -cf ${bundle} -P --exclude='*.pyc' --transform 's|${agentDir}|${verifierDir}|' ${agentDir}`,
        ],
        { encoding: "utf8" }
      );
      expect(pack.status).toBe(0);
      const extract = spawnSync(
        "bash",
        ["-c", artifactExtractCommand(artifacts, bundle)],
        { encoding: "utf8" }
      );
      expect(extract.stderr).toBe("");
      expect(extract.status).toBe(0);
      expect(existsSync(join(verifierDir, "deleted.py"))).toBe(false);
      expect(existsSync(join(verifierDir, "skipped.pyc"))).toBe(false);
      expect(Bun.file(join(verifierDir, "kept.py")).size).toBe(5);
      expect(existsSync(join(root, "verifier", "untouched.txt"))).toBe(true);
      expect(existsSync(bundle)).toBe(false);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("replaces a directory declared with a trailing slash", () => {
    const root = mkdtempSync(join(tmpdir(), "tb4-extract-"));
    try {
      const dir = join(root, "app");
      mkdirSync(dir, { recursive: true });
      writeFileSync(join(dir, "kept.py"), "agent");
      const artifacts = [`${dir}/`];
      const bundleCmd = artifactBundleCommand(artifacts);
      expect(bundleCmd).toContain(`'${dir}'`);
      expect(bundleCmd).not.toContain(`'${dir}/'`);
      const bundle = join(root, "bundle.tar");
      const pack = spawnSync("bash", ["-c", `tar -cf ${bundle} -P ${dir}/`], {
        encoding: "utf8",
      });
      expect(pack.status).toBe(0);
      writeFileSync(join(dir, "deleted.py"), "image");
      const extract = spawnSync(
        "bash",
        ["-c", artifactExtractCommand(artifacts, bundle)],
        { encoding: "utf8" }
      );
      expect(extract.stderr).toBe("");
      expect(extract.status).toBe(0);
      expect(existsSync(join(dir, "deleted.py"))).toBe(false);
      expect(existsSync(join(dir, "kept.py"))).toBe(true);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("leaves a destination alone when the bundle has no directory entry for it", () => {
    const root = mkdtempSync(join(tmpdir(), "tb4-extract-"));
    try {
      const verifierDir = join(root, "verifier", "pkg");
      mkdirSync(verifierDir, { recursive: true });
      writeFileSync(join(verifierDir, "image.py"), "image");
      const bundle = join(root, "bundle.tar");
      spawnSync("bash", ["-c", `tar -cf ${bundle} -T /dev/null`]);
      const extract = spawnSync(
        "bash",
        ["-c", artifactExtractCommand([verifierDir], bundle)],
        { encoding: "utf8" }
      );
      expect(extract.status).toBe(0);
      expect(existsSync(join(verifierDir, "image.py"))).toBe(true);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
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
