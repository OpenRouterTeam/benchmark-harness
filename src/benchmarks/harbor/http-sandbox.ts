import { readdirSync, statSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

import type { Effect } from "effect/Effect";
import {
  catchAll,
  fail,
  gen,
  tapError,
  tryPromise,
  void as effectVoid,
} from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed } from "effect/Layer";
import type { ZodType } from "zod";
import { object as zodObject, string as zodString } from "zod";

import type { SolverError } from "../../harness/core";
import { firstZodIssueMessage, parseSchema } from "../../internal/zod";
import type {
  CreateSandboxRequest,
  ExecStatusResponse,
  SandboxStatusResponse,
} from "./http-sandbox-protocol";
import {
  CreateSandboxResponseSchema,
  decodeHttpSandboxId,
  encodeHttpSandboxId,
  ErrorResponseSchema,
  ExecStatusResponseSchema,
  SandboxStatusResponseSchema,
  StartExecResponseSchema,
} from "./http-sandbox-protocol";
import type {
  CreateSessionInput,
  SandboxExec,
  SandboxSessionInstance,
} from "./sandbox";
import { SandboxSession, makeSessionInstance, toSolverError } from "./sandbox";

export interface HttpSandboxConfig {
  readonly baseUrl: string;
  readonly authToken: string;
  readonly allowedHostSuffixes?: readonly string[];
  readonly allowInsecureHttp?: boolean;
  readonly fetchFn?: typeof fetch;
  readonly pollIntervalMs?: number;
  readonly createTimeoutMs?: number;
  readonly capacityRetry?: {
    readonly maxAttempts: number;
    readonly delayMs: number;
  };
}

const DEFAULT_POLL_INTERVAL_MS = 1000;
const DEFAULT_CREATE_TIMEOUT_MS = 45 * 60 * 1000;
const DEFAULT_CAPACITY_RETRY_MAX_ATTEMPTS = 40;
const DEFAULT_CAPACITY_RETRY_DELAY_MS = 15_000;
const CONTROL_REQUEST_TIMEOUT_MS = 60_000;
const FILE_REQUEST_TIMEOUT_MS = 10 * 60 * 1000;
const EXEC_GRACE_MS = 60_000;

interface ResolvedConfig {
  readonly baseUrl: string;
  readonly authToken: string;
  readonly allowedHostSuffixes: readonly string[];
  readonly allowInsecureHttp: boolean;
  readonly fetchFn: typeof fetch;
  readonly pollIntervalMs: number;
  readonly createTimeoutMs: number;
  readonly capacityRetryMaxAttempts: number;
  readonly capacityRetryDelayMs: number;
}

class HttpSandboxError extends Error {
  readonly status: number | undefined;

  constructor(message: string, status?: number) {
    super(message);
    this.name = "HttpSandboxError";
    this.status = status;
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function stripTrailingSlash(url: string): string {
  return url.endsWith("/") ? url.slice(0, -1) : url;
}

async function errorMessageFrom(response: Response): Promise<string> {
  const bodyText = await response.text().catch(() => "");
  try {
    const parsed = parseSchema(ErrorResponseSchema, JSON.parse(bodyText));
    if (parsed._tag === "Right") {
      return parsed.right.error;
    }
  } catch {
    return bodyText.slice(0, 500);
  }
  return bodyText.slice(0, 500);
}

async function requestJson(
  config: ResolvedConfig,
  url: string,
  init: {
    readonly method: string;
    readonly body?: string | Uint8Array;
    readonly timeoutMs?: number;
  }
): Promise<unknown> {
  const response = await config.fetchFn(url, {
    method: init.method,
    headers: {
      authorization: `Bearer ${config.authToken}`,
      ...(typeof init.body === "string" && {
        "content-type": "application/json",
      }),
    },
    ...(init.body !== undefined && {
      body:
        typeof init.body === "string"
          ? init.body
          : new Blob([init.body.slice()]),
    }),
    signal: AbortSignal.timeout(init.timeoutMs ?? CONTROL_REQUEST_TIMEOUT_MS),
    redirect: "error",
  });
  if (!response.ok) {
    const message = await errorMessageFrom(response);
    throw new HttpSandboxError(
      `${init.method} ${url} failed with ${response.status}: ${message}`,
      response.status
    );
  }
  return response.json();
}

function parseResponse<Output>(
  schema: ZodType<Output>,
  value: unknown,
  context: string
): Output {
  const parsed = parseSchema(schema, value);
  if (parsed._tag === "Left") {
    throw new HttpSandboxError(
      `${context}: invalid host response: ${firstZodIssueMessage(parsed.left)}`
    );
  }
  return parsed.right;
}

function isCapacityStatus(status: number | undefined): boolean {
  return status === 429 || status === 503;
}

function assertAllowedHostUrl(config: ResolvedConfig, hostUrl: string): void {
  let parsed: URL;
  try {
    parsed = new URL(hostUrl);
  } catch {
    throw new HttpSandboxError(`host URL ${hostUrl} is not a valid URL`);
  }
  if (parsed.protocol !== "https:" && !config.allowInsecureHttp) {
    throw new HttpSandboxError(
      `host URL ${hostUrl} must use https (set allowInsecureHttp for private networks)`
    );
  }
  if (parsed.origin === new URL(config.baseUrl).origin) {
    return;
  }
  const isAllowed = config.allowedHostSuffixes.some(
    (suffix) =>
      parsed.hostname === suffix || parsed.hostname.endsWith(`.${suffix}`)
  );
  if (!isAllowed) {
    throw new HttpSandboxError(
      `host URL ${hostUrl} is not in the allowed host set`
    );
  }
}

const LeakedSandboxSchema = zodObject({ sandboxId: zodString() });

async function destroyLeakedSandbox(
  config: ResolvedConfig,
  raw: unknown
): Promise<void> {
  const parsed = parseSchema(LeakedSandboxSchema, raw);
  if (parsed._tag === "Left") {
    return;
  }
  await destroyOnHost(config, config.baseUrl, parsed.right.sandboxId).catch(
    () => undefined
  );
}

async function createOnHost(
  config: ResolvedConfig,
  request: CreateSandboxRequest
): Promise<{ hostUrl: string; localId: string }> {
  const url = `${config.baseUrl}/v1/sandboxes`;
  const body = JSON.stringify(request);
  let lastError: unknown;
  for (
    let attempt = 0;
    attempt < config.capacityRetryMaxAttempts;
    attempt += 1
  ) {
    let raw: unknown;
    try {
      raw = await requestJson(config, url, { method: "POST", body });
    } catch (error) {
      lastError = error;
      const isCapacity =
        error instanceof HttpSandboxError && isCapacityStatus(error.status);
      if (!isCapacity || attempt === config.capacityRetryMaxAttempts - 1) {
        throw error;
      }
      await sleep(config.capacityRetryDelayMs);
      continue;
    }
    try {
      const parsed = parseResponse(
        CreateSandboxResponseSchema,
        raw,
        "create sandbox"
      );
      const hostUrl = stripTrailingSlash(parsed.hostUrl);
      assertAllowedHostUrl(config, hostUrl);
      return { hostUrl, localId: parsed.sandboxId };
    } catch (error) {
      await destroyLeakedSandbox(config, raw);
      throw error;
    }
  }
  throw lastError ?? new HttpSandboxError("create sandbox: no attempts made");
}

async function getSandboxStatus(
  config: ResolvedConfig,
  hostUrl: string,
  localId: string
): Promise<SandboxStatusResponse> {
  const raw = await requestJson(
    config,
    `${hostUrl}/v1/sandboxes/${encodeURIComponent(localId)}`,
    { method: "GET" }
  );
  return parseResponse(SandboxStatusResponseSchema, raw, "sandbox status");
}

async function waitUntilRunning(
  config: ResolvedConfig,
  hostUrl: string,
  localId: string
): Promise<void> {
  const deadline = Date.now() + config.createTimeoutMs;
  for (;;) {
    const status = await getSandboxStatus(config, hostUrl, localId);
    if (status.status === "running") {
      return;
    }
    if (status.status === "failed" || status.status === "stopped") {
      throw new HttpSandboxError(
        `sandbox ${localId} entered ${status.status}: ${status.error ?? "no error reported"}`
      );
    }
    if (Date.now() >= deadline) {
      throw new HttpSandboxError(
        `sandbox ${localId} did not become running within ${config.createTimeoutMs}ms`
      );
    }
    await sleep(config.pollIntervalMs);
  }
}

async function execOnHost(
  config: ResolvedConfig,
  hostUrl: string,
  localId: string,
  argv: readonly string[],
  env: Readonly<Record<string, string>>,
  timeoutMs: number
): Promise<Extract<ExecStatusResponse, { status: "done" }>> {
  const startRaw = await requestJson(
    config,
    `${hostUrl}/v1/sandboxes/${encodeURIComponent(localId)}/execs`,
    {
      method: "POST",
      body: JSON.stringify({ argv: [...argv], env, timeoutMs }),
    }
  );
  const { execId } = parseResponse(StartExecResponseSchema, startRaw, "exec");
  const deadline = Date.now() + timeoutMs + EXEC_GRACE_MS;
  for (;;) {
    const statusRaw = await requestJson(
      config,
      `${hostUrl}/v1/sandboxes/${encodeURIComponent(localId)}/execs/${encodeURIComponent(execId)}`,
      { method: "GET" }
    );
    const status = parseResponse(
      ExecStatusResponseSchema,
      statusRaw,
      "exec status"
    );
    if (status.status === "done") {
      return status;
    }
    if (Date.now() >= deadline) {
      throw new HttpSandboxError(
        `exec ${execId} did not finish within ${timeoutMs}ms (+grace)`
      );
    }
    await sleep(config.pollIntervalMs);
  }
}

function fileUrl(hostUrl: string, localId: string, remotePath: string): string {
  return `${hostUrl}/v1/sandboxes/${encodeURIComponent(localId)}/files?path=${encodeURIComponent(remotePath)}`;
}

async function uploadFileToHost(
  config: ResolvedConfig,
  hostUrl: string,
  localId: string,
  localPath: string,
  remotePath: string
): Promise<void> {
  const bytes = await readFile(localPath);
  await requestJson(config, fileUrl(hostUrl, localId, remotePath), {
    method: "PUT",
    body: bytes,
    timeoutMs: FILE_REQUEST_TIMEOUT_MS,
  });
}

async function downloadFileFromHost(
  config: ResolvedConfig,
  hostUrl: string,
  localId: string,
  remotePath: string,
  localPath: string
): Promise<void> {
  const url = fileUrl(hostUrl, localId, remotePath);
  const response = await config.fetchFn(url, {
    method: "GET",
    headers: { authorization: `Bearer ${config.authToken}` },
    signal: AbortSignal.timeout(FILE_REQUEST_TIMEOUT_MS),
    redirect: "error",
  });
  if (!response.ok) {
    const message = await errorMessageFrom(response);
    throw new HttpSandboxError(
      `GET ${url} failed with ${response.status}: ${message}`,
      response.status
    );
  }
  const bytes = new Uint8Array(await response.arrayBuffer());
  await writeFile(localPath, bytes);
}

async function destroyOnHost(
  config: ResolvedConfig,
  hostUrl: string,
  localId: string
): Promise<void> {
  await requestJson(
    config,
    `${hostUrl}/v1/sandboxes/${encodeURIComponent(localId)}`,
    { method: "DELETE" }
  );
}

function makeInstance(
  config: ResolvedConfig,
  hostUrl: string,
  localId: string
): SandboxSessionInstance {
  const exec: SandboxExec = (argv, env, timeoutMs) =>
    tryPromise({
      try: async () => {
        const done = await execOnHost(
          config,
          hostUrl,
          localId,
          argv,
          env,
          timeoutMs
        );
        return {
          stdout: done.stdout,
          stderr: done.stderr,
          exitCode: done.exitCode,
        };
      },
      catch: (e) => toSolverError(`exec(${argv.join(" ")}) failed`, e),
    });
  return makeSessionInstance({
    sandboxId: encodeHttpSandboxId(hostUrl, localId),
    exec,
    uploadFile: (localPath, remotePath) =>
      uploadFileToHost(config, hostUrl, localId, localPath, remotePath),
    uploadDir: (localDir, remoteDir) =>
      uploadDir(config, hostUrl, localId, localDir, remoteDir),
    downloadFile: (remotePath, localPath) =>
      downloadFileFromHost(config, hostUrl, localId, remotePath, localPath),
    terminate: () => destroyOnHost(config, hostUrl, localId),
  });
}

export interface HttpSandboxService {
  readonly create: (
    sessionInput: CreateSessionInput
  ) => Effect<SandboxSessionInstance, SolverError>;
  readonly attach: (
    sandboxId: string
  ) => Effect<SandboxSessionInstance, SolverError>;
}

export function makeHttpSandboxService(
  input: HttpSandboxConfig
): HttpSandboxService {
  const config: ResolvedConfig = {
    baseUrl: stripTrailingSlash(input.baseUrl),
    authToken: input.authToken,
    allowedHostSuffixes: input.allowedHostSuffixes ?? [],
    allowInsecureHttp: input.allowInsecureHttp ?? false,
    fetchFn: input.fetchFn ?? fetch,
    pollIntervalMs: input.pollIntervalMs ?? DEFAULT_POLL_INTERVAL_MS,
    createTimeoutMs: input.createTimeoutMs ?? DEFAULT_CREATE_TIMEOUT_MS,
    capacityRetryMaxAttempts:
      input.capacityRetry?.maxAttempts ?? DEFAULT_CAPACITY_RETRY_MAX_ATTEMPTS,
    capacityRetryDelayMs:
      input.capacityRetry?.delayMs ?? DEFAULT_CAPACITY_RETRY_DELAY_MS,
  };
  const create = (
    sessionInput: CreateSessionInput
  ): Effect<SandboxSessionInstance, SolverError> =>
    gen(function* create() {
      const request: CreateSandboxRequest = {
        imageTag: sessionInput.imageTag,
        ...(sessionInput.imageBuildSteps !== undefined &&
          sessionInput.imageBuildSteps.length > 0 && {
            imageBuildSteps: [...sessionInput.imageBuildSteps],
          }),
        timeoutSec: sessionInput.timeoutSec,
        cpus: sessionInput.cpus,
        memoryMb: sessionInput.memoryMb,
        allowInternet: sessionInput.allowInternet,
        workdir: sessionInput.workdir,
        command: [...sessionInput.keepAliveCommand],
      };
      const { hostUrl, localId } = yield* tryPromise({
        try: () => createOnHost(config, request),
        catch: (e) => toSolverError("Failed to create HTTP sandbox", e),
      });
      const instance = makeInstance(config, hostUrl, localId);
      const setup = gen(function* setup() {
        yield* tryPromise({
          try: () => waitUntilRunning(config, hostUrl, localId),
          catch: (e) => toSolverError(`Sandbox ${localId} failed to start`, e),
        });
        for (const upload of sessionInput.uploads) {
          yield* upload.kind === "dir"
            ? tryPromise({
                try: () =>
                  uploadDir(
                    config,
                    hostUrl,
                    localId,
                    upload.localPath,
                    upload.remotePath
                  ),
                catch: (e) =>
                  toSolverError(`Failed to upload dir ${upload.localPath}`, e),
              })
            : instance.uploadFile(upload.localPath, upload.remotePath);
        }
        yield* instance.exec(
          ["mkdir", "-p", "/logs/verifier", "/logs/agent", "/logs/artifacts"],
          {},
          10_000
        );
        yield* instance.exec(["true"], {}, 30_000);
        return instance;
      });
      return yield* setup.pipe(
        tapError(() =>
          tryPromise({
            try: () => destroyOnHost(config, hostUrl, localId),
            catch: () => undefined,
          }).pipe(catchAll(() => effectVoid))
        )
      );
    });
  const attach = (
    sandboxId: string
  ): Effect<SandboxSessionInstance, SolverError> =>
    gen(function* attach() {
      const decoded = decodeHttpSandboxId(sandboxId);
      if (decoded === undefined) {
        return yield* fail(
          toSolverError(
            `Failed to reattach ${sandboxId}`,
            "malformed sandbox id"
          )
        );
      }
      try {
        assertAllowedHostUrl(config, decoded.hostUrl);
      } catch (error) {
        return yield* fail(
          toSolverError(`Failed to reattach ${sandboxId}`, error)
        );
      }
      const status = yield* tryPromise({
        try: () => getSandboxStatus(config, decoded.hostUrl, decoded.localId),
        catch: (e) => toSolverError(`Failed to reattach ${sandboxId}`, e),
      });
      if (status.status !== "running") {
        return yield* fail(
          toSolverError(
            `Failed to reattach ${sandboxId}`,
            `sandbox is ${status.status}: ${status.error ?? "not running"}`
          )
        );
      }
      const instance = makeInstance(config, decoded.hostUrl, decoded.localId);
      yield* instance.exec(["true"], {}, 30_000);
      return instance;
    });
  return { create, attach };
}

export function makeHttpSandboxLayer(
  input: HttpSandboxConfig
): Layer<SandboxSession> {
  return succeed(SandboxSession, makeHttpSandboxService(input));
}

async function uploadDir(
  config: ResolvedConfig,
  hostUrl: string,
  localId: string,
  localDir: string,
  remoteDir: string
): Promise<void> {
  for (const entry of readdirSync(localDir)) {
    const localPath = join(localDir, entry);
    const remotePath = join(remoteDir, entry);
    await (statSync(localPath).isDirectory()
      ? uploadDir(config, hostUrl, localId, localPath, remotePath)
      : uploadFileToHost(config, hostUrl, localId, localPath, remotePath));
  }
}
