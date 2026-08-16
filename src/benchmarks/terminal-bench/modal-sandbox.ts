import { readdirSync, statSync } from "node:fs";
import { join } from "node:path";

import type { Effect } from "effect/Effect";
import { catchAll, gen, tryPromise, void as effectVoid } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed } from "effect/Layer";
import type { App } from "modal";
import { ModalClient } from "modal";

import type { SolverError } from "../../harness/core";
import type {
  CreateSessionInput,
  SandboxExec,
  SandboxSessionInstance,
} from "./sandbox";
import {
  SandboxSession,
  CONTAINER_WORKDIR,
  REMOTE_INSTRUCTION,
  REMOTE_TEST_DIR,
  makeSessionInstance,
  toSolverError,
} from "./sandbox";

export interface ModalSandboxConfig {
  readonly environment?: string;
  readonly tokenId?: string;
  readonly tokenSecret?: string;
}

const TERMINAL_BENCH_APP = "openrouter-terminal-bench";

async function readStream(stream: AsyncIterable<string>): Promise<string> {
  let out = "";
  for await (const chunk of stream) {
    out += chunk;
  }
  return out;
}

export function makeModalSandboxLayer(
  config?: ModalSandboxConfig
): Layer<SandboxSession> {
  const environment = config?.environment ?? "main";
  const clientParams: ConstructorParameters<typeof ModalClient>[0] = {
    environment,
    ...(config?.tokenId !== undefined && { tokenId: config.tokenId }),
    ...(config?.tokenSecret !== undefined && {
      tokenSecret: config.tokenSecret,
    }),
  };
  const client = new ModalClient(clientParams);
  let appPromise: Promise<App> | undefined;
  const getApp = () => {
    if (!appPromise) {
      appPromise = client.apps
        .fromName(TERMINAL_BENCH_APP, { createIfMissing: true, environment })
        .catch((error: unknown) => {
          appPromise = undefined;
          throw error;
        });
    }
    return appPromise;
  };
  const create = (
    input: CreateSessionInput
  ): Effect<SandboxSessionInstance, SolverError> =>
    gen(function* create() {
      const app = yield* tryPromise({
        try: () => getApp(),
        catch: (e) => toSolverError("Failed to resolve Modal app", e),
      });
      const baseImage = client.images.fromRegistry(input.imageTag);
      const image =
        input.imageBuildSteps !== undefined && input.imageBuildSteps.length > 0
          ? baseImage.dockerfileCommands([...input.imageBuildSteps])
          : baseImage;
      const builtImage = yield* tryPromise({
        try: () => image.build(app),
        catch: (e: unknown) =>
          toSolverError(`Failed to build layered image ${input.imageTag}`, e),
      });
      const sandboxTimeoutMs =
        (input.maxAgentTimeoutSec + input.maxTestTimeoutSec + 300) * 1000;
      const sandbox = yield* tryPromise({
        try: () =>
          client.sandboxes.create(app, builtImage, {
            timeoutMs: sandboxTimeoutMs,
            workdir: CONTAINER_WORKDIR,
            command: ["sleep", "infinity"],
          }),
        catch: (e) => toSolverError("Failed to create Modal sandbox", e),
      });
      let handedOff = false;
      try {
        yield* tryPromise({
          try: () =>
            uploadTaskFiles(sandbox.filesystem, {
              testDir: input.testDir,
              testScript: input.testScript,
              instructionPath: input.instructionPath,
            }),
          catch: (e) =>
            toSolverError("Failed to upload task files into sandbox", e),
        });
        yield* tryPromise({
          try: () =>
            sandbox
              .exec(["mkdir", "-p", "/logs/verifier"], {
                mode: "text",
                timeoutMs: 10000,
              })
              .then((p) => p.wait()),
          catch: (e) =>
            toSolverError("Failed to create /logs/verifier in sandbox", e),
        });
        const exec: SandboxExec = (argv, env, timeoutMs) =>
          gen(function* exec() {
            const proc = yield* tryPromise({
              try: () =>
                sandbox.exec([...argv], { mode: "text", timeoutMs, env }),
              catch: (e) => toSolverError(`exec(${argv.join(" ")}) failed`, e),
            });
            const [stdout, stderr, exitCode] = yield* tryPromise({
              try: () =>
                Promise.all([
                  readStream(proc.stdout),
                  readStream(proc.stderr),
                  proc.wait(),
                ]),
              catch: (e) =>
                toSolverError(`exec(${argv.join(" ")}) read failed`, e),
            });
            return { stdout, stderr, exitCode };
          });
        yield* exec(["true"], {}, 30000);
        const instance = makeSessionInstance({
          sandboxId: sandbox.sandboxId,
          exec,
          maxTestTimeoutSec: input.maxTestTimeoutSec,
          terminate: () => sandbox.terminate(),
        });
        handedOff = true;
        return instance;
      } finally {
        if (!handedOff) {
          yield* tryPromise({
            try: () => sandbox.terminate(),
            catch: () => Promise.resolve(),
          }).pipe(catchAll(() => effectVoid));
        }
      }
    });
  return succeed(SandboxSession, { create });
}

interface UploadTaskFilesInput {
  readonly testDir: string;
  readonly testScript: string;
  readonly instructionPath: string;
}

async function uploadTaskFiles(
  fs: {
    copyFromLocal: (localPath: string, remotePath: string) => Promise<void>;
  },
  input: UploadTaskFilesInput
): Promise<void> {
  await fs.copyFromLocal(input.instructionPath, REMOTE_INSTRUCTION);
  await fs.copyFromLocal(input.testScript, `${REMOTE_TEST_DIR}/test.sh`);
  await uploadDir(fs, input.testDir, REMOTE_TEST_DIR);
}

async function uploadDir(
  fs: {
    copyFromLocal: (localPath: string, remotePath: string) => Promise<void>;
  },
  localDir: string,
  remoteDir: string
): Promise<void> {
  for (const entry of readdirSync(localDir)) {
    const localPath = join(localDir, entry);
    const remotePath = join(remoteDir, entry);
    if (statSync(localPath).isDirectory()) {
      await uploadDir(fs, localPath, remotePath);
    } else {
      await fs.copyFromLocal(localPath, remotePath);
    }
  }
}
