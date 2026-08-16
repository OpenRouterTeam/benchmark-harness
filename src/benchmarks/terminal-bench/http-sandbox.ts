import type { Effect } from "effect/Effect";
import { gen, runPromise } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed } from "effect/Layer";

import type { SolverError } from "../../harness/core";
import type { HttpSandboxConfig } from "../harbor/http-sandbox";
import { makeHttpSandboxService } from "../harbor/http-sandbox";
import type { CreateSessionInput, SandboxSessionInstance } from "./sandbox";
import {
  SandboxSession,
  CONTAINER_WORKDIR,
  REMOTE_INSTRUCTION,
  REMOTE_TEST_DIR,
  makeSessionInstance,
} from "./sandbox";

export interface TerminalBenchHttpSandboxConfig extends HttpSandboxConfig {
  readonly cpus?: number;
  readonly memoryMb?: number;
}

const DEFAULT_CPUS = 2;
const DEFAULT_MEMORY_MB = 4096;
const SANDBOX_OVERHEAD_SEC = 300;

export function makeHttpSandboxLayer(
  config: TerminalBenchHttpSandboxConfig
): Layer<SandboxSession> {
  const service = makeHttpSandboxService(config);
  const create = (
    input: CreateSessionInput
  ): Effect<SandboxSessionInstance, SolverError> =>
    gen(function* create() {
      const harborInstance = yield* service.create({
        imageTag: input.imageTag,
        ...(input.imageBuildSteps !== undefined &&
          input.imageBuildSteps.length > 0 && {
            imageBuildSteps: input.imageBuildSteps,
          }),
        timeoutSec:
          input.maxAgentTimeoutSec +
          input.maxTestTimeoutSec +
          SANDBOX_OVERHEAD_SEC,
        cpus: config.cpus ?? DEFAULT_CPUS,
        memoryMb: config.memoryMb ?? DEFAULT_MEMORY_MB,
        allowInternet: true,
        workdir: CONTAINER_WORKDIR,
        keepAliveCommand: ["sleep", "infinity"],
        uploads: [
          {
            localPath: input.instructionPath,
            remotePath: REMOTE_INSTRUCTION,
            kind: "file",
          },
          {
            localPath: input.testScript,
            remotePath: `${REMOTE_TEST_DIR}/test.sh`,
            kind: "file",
          },
          {
            localPath: input.testDir,
            remotePath: REMOTE_TEST_DIR,
            kind: "dir",
          },
        ],
      });
      return makeSessionInstance({
        sandboxId: harborInstance.sandboxId,
        exec: harborInstance.exec,
        maxTestTimeoutSec: input.maxTestTimeoutSec,
        terminate: () => runPromise(harborInstance.destroy()),
      });
    });
  return succeed(SandboxSession, { create });
}
