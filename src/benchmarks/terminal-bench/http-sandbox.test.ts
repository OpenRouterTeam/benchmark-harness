import { describe, expect, it } from "bun:test";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { gen, provide, runPromise } from "effect/Effect";

import { makeHttpSandboxLayer } from "./http-sandbox";
import type { CreateSessionInput } from "./sandbox";
import { SandboxSession } from "./sandbox";

const BASE_URL = "http://sandbox-lb.internal";
const HOST_URL = "http://sandbox-host-1.internal";

interface FakeHost {
  readonly fetchFn: typeof fetch;
  readonly createBodies: unknown[];
  readonly execs: string[][];
  readonly uploadedPaths: string[];
  readonly destroyed: string[];
}

function makeFakeHost(): FakeHost {
  const createBodies: unknown[] = [];
  const execs: string[][] = [];
  const uploadedPaths: string[] = [];
  const destroyed: string[] = [];
  let execCounter = 0;
  const json = (body: unknown, status = 200): Response =>
    new Response(JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    });
  const fetchFn = (async (
    input: string | URL | Request,
    init?: RequestInit
  ): Promise<Response> => {
    const url = new URL(String(input));
    const method = init?.method ?? "GET";
    if (method === "POST" && url.pathname === "/v1/sandboxes") {
      createBodies.push(JSON.parse(String(init?.body)));
      return json({ sandboxId: "sbx-1", hostUrl: HOST_URL });
    }
    if (method === "GET" && url.pathname === "/v1/sandboxes/sbx-1") {
      return json({ sandboxId: "sbx-1", status: "running" });
    }
    if (method === "DELETE" && url.pathname === "/v1/sandboxes/sbx-1") {
      destroyed.push("sbx-1");
      return json({ sandboxId: "sbx-1", status: "stopped" });
    }
    if (method === "POST" && url.pathname.endsWith("/execs")) {
      const body = JSON.parse(String(init?.body)) as { argv: string[] };
      execs.push(body.argv);
      execCounter += 1;
      return json({ execId: `exec-${execCounter}` });
    }
    if (method === "GET" && /\/execs\/[^/]+$/.test(url.pathname)) {
      const lastArgv = execs.at(-1) ?? [];
      const stdout = lastArgv[0] === "cat" ? "1" : "test output";
      return json({ status: "done", stdout, stderr: "", exitCode: 0 });
    }
    if (method === "PUT" && url.pathname.endsWith("/files")) {
      uploadedPaths.push(url.searchParams.get("path") ?? "");
      return json({ ok: true });
    }
    return json({ error: `unhandled ${method} ${url.pathname}` }, 500);
  }) as typeof fetch;
  return { fetchFn, createBodies, execs, uploadedPaths, destroyed };
}

function makeTaskFiles(): CreateSessionInput {
  const dir = mkdtempSync(join(tmpdir(), "tb-http-sandbox-"));
  const instructionPath = join(dir, "instruction.md");
  writeFileSync(instructionPath, "solve the task");
  const testDir = join(dir, "tests");
  mkdirSync(testDir);
  writeFileSync(join(testDir, "test_case.py"), "assert True");
  const testScript = join(dir, "run-tests.sh");
  writeFileSync(testScript, "pytest");
  return {
    imageTag: "ghcr.io/example/task:latest",
    maxAgentTimeoutSec: 600,
    maxTestTimeoutSec: 120,
    testDir,
    testScript,
    instructionPath,
  };
}

function createSession(host: FakeHost, input: CreateSessionInput) {
  return runPromise(
    gen(function* () {
      const session = yield* SandboxSession;
      return yield* session.create(input);
    }).pipe(
      provide(
        makeHttpSandboxLayer({
          baseUrl: BASE_URL,
          authToken: "token",
          allowedHostSuffixes: ["sandbox-host-1.internal"],
          fetchFn: host.fetchFn,
          pollIntervalMs: 1,
          createTimeoutMs: 5000,
        })
      )
    )
  );
}

describe("terminal-bench makeHttpSandboxLayer", () => {
  it("maps the task config onto the harbor create request", async () => {
    const host = makeFakeHost();

    await createSession(host, makeTaskFiles());

    expect(host.createBodies).toHaveLength(1);
    expect(host.createBodies[0]).toMatchObject({
      imageTag: "ghcr.io/example/task:latest",
      timeoutSec: 600 + 120 + 300,
      allowInternet: true,
      workdir: "/app",
      command: ["sleep", "infinity"],
    });
  });

  it("uploads instruction, test script, and test dir to the task paths", async () => {
    const host = makeFakeHost();

    await createSession(host, makeTaskFiles());

    expect(host.uploadedPaths).toEqual([
      "/instruction.md",
      "/tests/test.sh",
      "/tests/test_case.py",
    ]);
  });

  it("runs tests via the uploaded test script and reads the reward", async () => {
    const host = makeFakeHost();
    const instance = await createSession(host, makeTaskFiles());

    const result = await runPromise(instance.runTests());

    expect(result.reward).toBe(1);
    const testExec = host.execs.find((argv) =>
      argv.join(" ").includes("/tests/test.sh")
    );
    expect(testExec).toBeDefined();
  });

  it("destroys the sandbox on the host", async () => {
    const host = makeFakeHost();
    const instance = await createSession(host, makeTaskFiles());

    await runPromise(instance.destroy());

    expect(host.destroyed).toEqual(["sbx-1"]);
  });
});
