import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { Effect } from "effect/Effect";
import { fail, gen, succeed } from "effect/Effect";

import { SolverError } from "../../harness/core";
import { parseReward } from "../harbor/reward";
import type {
  SandboxSessionFactory,
  SandboxSessionInstance,
} from "../harbor/sandbox";
import { REMOTE_VERIFIER_SCRIPT, SANDBOX_IMAGE_KINDS } from "../harbor/sandbox";
import type { TerminalBench4SampleMeta } from "./dataset";
import type { Artifact, CollectHook } from "./schema";

export const CONTAINER_WORKDIR = "/app" as const;

export const REMOTE_INSTRUCTION = "/instruction.md" as const;

export const REMOTE_REWARD_PATH = "/logs/verifier/reward.txt" as const;

export const REMOTE_ARTIFACTS_DIR = "/logs/artifacts" as const;

export const KEEP_ALIVE_COMMAND = ["sleep", "infinity"] as const;

const REMOTE_ARTIFACT_BUNDLE = "/tmp/tb4-artifacts.tar" as const;

export const SANDBOX_TIMEOUT_MARGIN_SEC = 300;

const VERIFIER_TIMEOUT_MARGIN_MS = 5000;

const REWARD_READ_TIMEOUT_MS = 10_000;

export const ARTIFACT_BUNDLE_TIMEOUT_MS = 600_000;

export const ARTIFACT_TRANSFER_TIMEOUT_MS = 1_800_000;

export const DEFAULT_COLLECT_TIMEOUT_SEC = 300;

const ARTIFACT_PHASE_SEC =
  (2 * ARTIFACT_BUNDLE_TIMEOUT_MS + ARTIFACT_TRANSFER_TIMEOUT_MS) / 1000;

export function agentSandboxTimeoutSec(meta: TerminalBench4SampleMeta): number {
  const collectSec = meta.collect.reduce(
    (total, hook) => total + (hook.timeout_sec ?? DEFAULT_COLLECT_TIMEOUT_SEC),
    0
  );
  return Math.ceil(
    meta.maxAgentTimeoutSec +
      collectSec +
      ARTIFACT_PHASE_SEC +
      SANDBOX_TIMEOUT_MARGIN_SEC
  );
}

export function verifierSandboxTimeoutSec(
  meta: TerminalBench4SampleMeta
): number {
  return Math.ceil(
    meta.maxTestTimeoutSec + ARTIFACT_PHASE_SEC + SANDBOX_TIMEOUT_MARGIN_SEC
  );
}

export function agentNetworkDeviation(
  meta: TerminalBench4SampleMeta
): Readonly<Record<string, unknown>> {
  return meta.agentEnv.allowInternet
    ? {}
    : { agentNetworkForced: true, taskAllowInternet: false };
}

export function createAgentSession(input: {
  readonly sessionFactory: SandboxSessionFactory;
  readonly meta: TerminalBench4SampleMeta;
  readonly tasksDir: string;
  readonly imageTag: string;
  readonly imageBuildSteps: readonly string[];
}): Effect<SandboxSessionInstance, SolverError> {
  const { sessionFactory, meta, tasksDir, imageTag, imageBuildSteps } = input;
  return sessionFactory.create({
    imageTag,
    imageKind: SANDBOX_IMAGE_KINDS.ModalImageId,
    imageBuildSteps,
    timeoutSec: agentSandboxTimeoutSec(meta),
    ...meta.agentEnv,
    allowInternet: true,
    workdir: CONTAINER_WORKDIR,
    keepAliveCommand: KEEP_ALIVE_COMMAND,
    uploads: [
      {
        localPath: join(tasksDir, meta.taskId, "instruction.md"),
        remotePath: REMOTE_INSTRUCTION,
        kind: "file",
      },
    ],
  });
}

export function createVerifierSession(input: {
  readonly sessionFactory: SandboxSessionFactory;
  readonly meta: TerminalBench4SampleMeta;
  readonly imageTag: string;
}): Effect<SandboxSessionInstance, SolverError> {
  const { sessionFactory, meta, imageTag } = input;
  return sessionFactory.create({
    imageTag,
    imageKind: SANDBOX_IMAGE_KINDS.ModalImageId,
    timeoutSec: verifierSandboxTimeoutSec(meta),
    ...meta.verifierEnv,
    workdir: CONTAINER_WORKDIR,
    keepAliveCommand: KEEP_ALIVE_COMMAND,
    uploads: [],
  });
}

export function sandboxCollectHooks(
  hooks: readonly CollectHook[]
): Effect<readonly CollectHook[], SolverError> {
  const sidecar = hooks.find(
    (h) => h.service !== undefined && h.service !== "main"
  );
  if (sidecar !== undefined) {
    return fail(
      new SolverError({
        message: `collect hook targets compose service "${sidecar.service}", which is unsupported`,
      })
    );
  }
  return succeed(hooks);
}

export function runCollectHooks(
  session: SandboxSessionInstance,
  hooks: readonly CollectHook[]
): Effect<void, SolverError> {
  return gen(function* () {
    for (const hook of hooks) {
      yield* session.exec(
        ["bash", "-c", hook.command],
        {},
        Math.round((hook.timeout_sec ?? DEFAULT_COLLECT_TIMEOUT_SEC) * 1000)
      );
    }
  });
}

function artifactSource(artifact: Artifact): string {
  return typeof artifact === "string" ? artifact : artifact.source;
}

function artifactExcludes(artifact: Artifact): readonly string[] {
  return typeof artifact === "string" ? [] : artifact.exclude;
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", String.raw`'\''`)}'`;
}

function artifactSources(artifacts: readonly Artifact[]): readonly string[] {
  return [REMOTE_ARTIFACTS_DIR, ...artifacts.map(artifactSource)].filter(
    (s, i, all) => all.indexOf(s) === i
  );
}

export function artifactBundleCommand(artifacts: readonly Artifact[]): string {
  const sources = artifactSources(artifacts);
  const excludes = artifacts
    .flatMap(artifactExcludes)
    .filter((s, i, all) => all.indexOf(s) === i)
    .map((pattern) => `--exclude=${shellQuote(pattern)}`);
  const existing = sources
    .map(
      (s) =>
        `if [ -e ${shellQuote(s)} ]; then printf '%s\\n' ${shellQuote(s)}; fi`
    )
    .join("; ");
  return [
    `rm -f ${REMOTE_ARTIFACT_BUNDLE}`,
    `{ ${existing}; } > /tmp/tb4-artifact-list.txt`,
    `if [ -s /tmp/tb4-artifact-list.txt ]; then tar -cf ${REMOTE_ARTIFACT_BUNDLE} -P ${excludes.join(" ")} -T /tmp/tb4-artifact-list.txt; else tar -cf ${REMOTE_ARTIFACT_BUNDLE} -T /dev/null; fi`,
  ].join(" && ");
}

export function artifactExtractCommand(
  artifacts: readonly Artifact[],
  bundle: string = REMOTE_ARTIFACT_BUNDLE
): string {
  const clearDirs = artifactSources(artifacts).map(
    (s) =>
      `if tar -tf ${bundle} -P | grep -qx ${shellQuote(`${s}/`)}; then rm -rf ${shellQuote(s)}; fi`
  );
  return [...clearDirs, `tar -xf ${bundle} -P -C /`, `rm -f ${bundle}`].join(
    " && "
  );
}

export function transferArtifacts(input: {
  readonly agent: SandboxSessionInstance;
  readonly verifier: SandboxSessionInstance;
  readonly artifacts: readonly Artifact[];
}): Effect<void, SolverError> {
  const { agent, verifier, artifacts } = input;
  return gen(function* () {
    yield* agent.exec(
      ["bash", "-c", artifactBundleCommand(artifacts)],
      {},
      ARTIFACT_BUNDLE_TIMEOUT_MS
    );
    const staging = mkdtempSync(join(tmpdir(), "tb4-artifacts-"));
    const localBundle = join(staging, "artifacts.tar");
    try {
      yield* agent.downloadFile(REMOTE_ARTIFACT_BUNDLE, localBundle);
      yield* verifier.uploadFile(localBundle, REMOTE_ARTIFACT_BUNDLE);
    } finally {
      rmSync(staging, { recursive: true, force: true });
    }
    yield* verifier.exec(
      ["bash", "-c", artifactExtractCommand(artifacts)],
      {},
      ARTIFACT_BUNDLE_TIMEOUT_MS
    );
  });
}

export interface TerminalBench4VerifierResult {
  readonly reward: number;
  readonly output: string;
}

export function runVerifier(
  verifier: SandboxSessionInstance,
  meta: TerminalBench4SampleMeta
): Effect<TerminalBench4VerifierResult, SolverError> {
  const verifierTimeoutMs =
    Math.round(meta.maxTestTimeoutSec * 1000) + VERIFIER_TIMEOUT_MARGIN_MS;
  return gen(function* () {
    const run = yield* verifier.exec(
      [
        "bash",
        "-c",
        `mkdir -p /logs/verifier && bash ${REMOTE_VERIFIER_SCRIPT}`,
      ],
      {},
      verifierTimeoutMs
    );
    const rewardRead = yield* verifier.exec(
      ["cat", REMOTE_REWARD_PATH],
      {},
      REWARD_READ_TIMEOUT_MS
    );
    return {
      reward: parseReward(rewardRead.stdout),
      output: `${run.stdout}\n${run.stderr}`.trim(),
    };
  });
}
