import { Tag } from "effect/Context";
import type { Effect } from "effect/Effect";
import {
  catchAll,
  succeed,
  sync,
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

export interface UploadSpec {
  readonly localPath: string;
  readonly remotePath: string;
  readonly kind: "file" | "dir";
}

export interface SandboxSessionInstance {
  readonly sandboxId: string;
  readonly exec: (
    argv: string[],
    env: Readonly<Record<string, string>>,
    timeoutMs: number
  ) => Effect<ExecResult, SolverError>;
  readonly uploadFile: (
    localPath: string,
    remotePath: string
  ) => Effect<void, SolverError>;
  readonly uploadDir: (
    localDir: string,
    remoteDir: string
  ) => Effect<void, SolverError>;
  readonly downloadFile: (
    remotePath: string,
    localPath: string
  ) => Effect<void, SolverError>;
  readonly destroy: () => Effect<void, SolverError>;
}

export const SANDBOX_IMAGE_KINDS = {
  Registry: "registry",
  ModalImageId: "modal-image-id",
} as const;

export type SandboxImageKind =
  (typeof SANDBOX_IMAGE_KINDS)[keyof typeof SANDBOX_IMAGE_KINDS];

export interface CreateSessionInput {
  readonly imageTag: string;
  readonly imageKind?: SandboxImageKind;
  readonly imageBuildSteps?: readonly string[];
  readonly timeoutSec: number;
  readonly cpus: number;
  readonly memoryMb: number;
  readonly gpu?: string;
  readonly env?: Readonly<Record<string, string>>;
  readonly allowInternet: boolean;
  readonly workdir: string;
  readonly keepAliveCommand: readonly string[];
  readonly uploads: readonly UploadSpec[];
}

export class SandboxSession extends Tag(
  "@openrouter/bench-harness/benchmarks/harbor/sandbox/SandboxSession"
)<
  SandboxSession,
  {
    readonly create: (
      input: CreateSessionInput
    ) => Effect<SandboxSessionInstance, SolverError>;
    readonly attach: (
      sandboxId: string
    ) => Effect<SandboxSessionInstance, SolverError>;
  }
>() {}

export type SandboxSessionFactory = {
  readonly create: (
    input: CreateSessionInput
  ) => Effect<SandboxSessionInstance, SolverError>;
  readonly attach: (
    sandboxId: string
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
  readonly uploadFile: (localPath: string, remotePath: string) => Promise<void>;
  readonly uploadDir: (localDir: string, remoteDir: string) => Promise<void>;
  readonly downloadFile: (
    remotePath: string,
    localPath: string
  ) => Promise<void>;
  readonly terminate: () => Promise<unknown>;
}

export function makeSessionInstance(
  input: MakeSessionInstanceInput
): SandboxSessionInstance {
  return {
    sandboxId: input.sandboxId,
    exec: input.exec,
    uploadFile: (localPath, remotePath) =>
      tryPromise({
        try: () => input.uploadFile(localPath, remotePath),
        catch: (e) =>
          toSolverError(`Failed to upload ${localPath} to ${remotePath}`, e),
      }),
    uploadDir: (localDir, remoteDir) =>
      tryPromise({
        try: () => input.uploadDir(localDir, remoteDir),
        catch: (e) =>
          toSolverError(`Failed to upload ${localDir} to ${remoteDir}`, e),
      }),
    downloadFile: (remotePath, localPath) =>
      tryPromise({
        try: () => input.downloadFile(remotePath, localPath),
        catch: (e) =>
          toSolverError(`Failed to download ${remotePath} to ${localPath}`, e),
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

export const REMOTE_VERIFIER_SCRIPT = "/tests/test.sh" as const;

export interface FakeSandboxBehavior {
  readonly execHandler: (
    argv: readonly string[],
    env: Readonly<Record<string, string>>,
    timeoutMs?: number
  ) => ExecResult;
  readonly onCreate?: (input: CreateSessionInput) => void;
  readonly onUploadFile?: (localPath: string, remotePath: string) => void;
  readonly onUploadDir?: (localDir: string, remoteDir: string) => void;
  readonly onDownloadFile?: (remotePath: string, localPath: string) => void;
  readonly onDestroy?: () => void;
}

export function makeFakeSandboxLayer(
  behavior: FakeSandboxBehavior
): Layer<SandboxSession> {
  const create = (
    input: CreateSessionInput
  ): Effect<SandboxSessionInstance, SolverError> => {
    behavior.onCreate?.(input);
    return succeed({
      sandboxId: "fake-sandbox",
      exec: (argv, env, timeoutMs) =>
        sync(() => behavior.execHandler(argv, env, timeoutMs)),
      uploadFile: (localPath, remotePath) =>
        sync(() => behavior.onUploadFile?.(localPath, remotePath)),
      uploadDir: (localDir, remoteDir) =>
        sync(() => behavior.onUploadDir?.(localDir, remoteDir)),
      downloadFile: (remotePath, localPath) =>
        sync(() => behavior.onDownloadFile?.(remotePath, localPath)),
      destroy: () => sync(() => behavior.onDestroy?.()),
    });
  };
  const attach = (
    sandboxId: string
  ): Effect<SandboxSessionInstance, SolverError> =>
    succeed({
      sandboxId,
      exec: (argv, env, timeoutMs) =>
        sync(() => behavior.execHandler(argv, env, timeoutMs)),
      uploadFile: (localPath, remotePath) =>
        sync(() => behavior.onUploadFile?.(localPath, remotePath)),
      uploadDir: (localDir, remoteDir) =>
        sync(() => behavior.onUploadDir?.(localDir, remoteDir)),
      downloadFile: (remotePath, localPath) =>
        sync(() => behavior.onDownloadFile?.(remotePath, localPath)),
      destroy: () => sync(() => behavior.onDestroy?.()),
    });
  return layerSucceed(SandboxSession, { create, attach });
}
