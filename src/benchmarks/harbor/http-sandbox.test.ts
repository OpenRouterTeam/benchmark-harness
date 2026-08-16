import { describe, expect, it } from "bun:test";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { gen, provide, runPromise, runPromiseExit } from "effect/Effect";

import { makeHttpSandboxLayer } from "./http-sandbox";
import { decodeHttpSandboxId } from "./http-sandbox-protocol";
import type { CreateSessionInput } from "./sandbox";
import { SandboxSession } from "./sandbox";

const BASE_URL = "http://sandbox-lb.internal";
const HOST_URL = "http://sandbox-host-1.internal";
const AUTH_TOKEN = "test-token";

interface FakeHostOptions {
  readonly creatingPolls?: number;
  readonly createRejections?: number;
  readonly createOutcome?: "running" | "failed" | "stopped";
  readonly execExitCode?: number;
  readonly hostUrlOverride?: string;
}

interface RecordedRequest {
  readonly method: string;
  readonly path: string;
  readonly authorization: string | null;
  readonly redirect: RequestRedirect | undefined;
}

interface FakeHost {
  readonly fetchFn: typeof fetch;
  readonly requests: RecordedRequest[];
  readonly execs: { argv: string[]; env: Record<string, string> }[];
  readonly files: Map<string, Uint8Array>;
  readonly destroyed: string[];
}

function makeFakeHost(options: FakeHostOptions = {}): FakeHost {
  const requests: RecordedRequest[] = [];
  const execs: { argv: string[]; env: Record<string, string> }[] = [];
  const files = new Map<string, Uint8Array>();
  const destroyed: string[] = [];
  let createRejectionsLeft = options.createRejections ?? 0;
  let statusPollsLeft = options.creatingPolls ?? 0;
  const outcome = options.createOutcome ?? "running";
  let sandboxCounter = 0;
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
    const headers = new Headers(init?.headers);
    requests.push({
      method,
      path: url.pathname,
      authorization: headers.get("authorization"),
      redirect: init?.redirect,
    });
    if (method === "POST" && url.pathname === "/v1/sandboxes") {
      if (createRejectionsLeft > 0) {
        createRejectionsLeft -= 1;
        return json({ error: "no capacity" }, 429);
      }
      sandboxCounter += 1;
      return json({
        sandboxId: `sbx-${sandboxCounter}`,
        hostUrl: options.hostUrlOverride ?? HOST_URL,
      });
    }
    const sandboxMatch = url.pathname.match(/^\/v1\/sandboxes\/([^/]+)$/);
    if (sandboxMatch?.[1] !== undefined && method === "GET") {
      if (statusPollsLeft > 0) {
        statusPollsLeft -= 1;
        return json({ sandboxId: sandboxMatch[1], status: "creating" });
      }
      return json({
        sandboxId: sandboxMatch[1],
        status: outcome,
        ...(outcome === "failed" && { error: "image pull failed" }),
      });
    }
    if (sandboxMatch?.[1] !== undefined && method === "DELETE") {
      destroyed.push(sandboxMatch[1]);
      return json({ sandboxId: sandboxMatch[1], status: "stopped" });
    }
    if (method === "POST" && url.pathname.endsWith("/execs")) {
      const body = JSON.parse(String(init?.body)) as {
        argv: string[];
        env: Record<string, string>;
      };
      execs.push({ argv: body.argv, env: body.env });
      execCounter += 1;
      return json({ execId: `exec-${execCounter}` });
    }
    if (method === "GET" && /\/execs\/[^/]+$/.test(url.pathname)) {
      return json({
        status: "done",
        stdout: "out",
        stderr: "err",
        exitCode: options.execExitCode ?? 0,
      });
    }
    if (url.pathname.endsWith("/files")) {
      const path = url.searchParams.get("path") ?? "";
      if (method === "PUT") {
        const body = init?.body;
        const bytes =
          body instanceof Blob
            ? new Uint8Array(await body.arrayBuffer())
            : new TextEncoder().encode(String(body));
        files.set(path, bytes);
        return json({ ok: true });
      }
      const content = files.get(path);
      if (content === undefined) {
        return json({ error: "not found" }, 404);
      }
      return new Response(new Blob([new Uint8Array(content)]), {
        status: 200,
      });
    }
    return json({ error: `unhandled ${method} ${url.pathname}` }, 500);
  }) as typeof fetch;
  return { fetchFn, requests, execs, files, destroyed };
}

function makeLayer(
  host: FakeHost,
  overrides: { readonly allowInsecureHttp?: boolean } = {}
): ReturnType<typeof makeHttpSandboxLayer> {
  return makeHttpSandboxLayer({
    baseUrl: BASE_URL,
    authToken: AUTH_TOKEN,
    allowedHostSuffixes: ["sandbox-host-1.internal"],
    allowInsecureHttp: overrides.allowInsecureHttp ?? true,
    fetchFn: host.fetchFn,
    pollIntervalMs: 1,
    createTimeoutMs: 5000,
    capacityRetry: { maxAttempts: 3, delayMs: 1 },
  });
}

const CREATE_INPUT: CreateSessionInput = {
  imageTag: "python:3.12",
  timeoutSec: 600,
  cpus: 2,
  memoryMb: 4096,
  allowInternet: false,
  workdir: "/app",
  keepAliveCommand: ["sleep", "infinity"],
  uploads: [],
};

function createSession(host: FakeHost, input: CreateSessionInput) {
  return runPromise(
    gen(function* () {
      const session = yield* SandboxSession;
      return yield* session.create(input);
    }).pipe(provide(makeLayer(host)))
  );
}

function createSessionExit(
  host: FakeHost,
  overrides: { readonly allowInsecureHttp?: boolean } = {}
) {
  return runPromiseExit(
    gen(function* () {
      const session = yield* SandboxSession;
      return yield* session.create(CREATE_INPUT);
    }).pipe(provide(makeLayer(host, overrides)))
  );
}

describe("makeHttpSandboxLayer create", () => {
  it("creates a sandbox, waits for running, and probes it", async () => {
    const host = makeFakeHost({ creatingPolls: 2 });

    const instance = await createSession(host, CREATE_INPUT);

    expect(decodeHttpSandboxId(instance.sandboxId)).toEqual({
      hostUrl: HOST_URL,
      localId: "sbx-1",
    });
    expect(host.execs.map((e) => e.argv[0])).toEqual(["mkdir", "true"]);
  });

  it("sends the bearer token on every request", async () => {
    const host = makeFakeHost();

    await createSession(host, CREATE_INPUT);

    const tokens = new Set(host.requests.map((r) => r.authorization));
    expect(tokens).toEqual(new Set([`Bearer ${AUTH_TOKEN}`]));
  });

  it("refuses to follow redirects on every request", async () => {
    const host = makeFakeHost();

    await createSession(host, CREATE_INPUT);

    const redirects = new Set(host.requests.map((r) => r.redirect));
    expect(redirects).toEqual(new Set(["error"]));
  });

  it("retries create when the pool is at capacity", async () => {
    const host = makeFakeHost({ createRejections: 2 });

    const instance = await createSession(host, CREATE_INPUT);

    expect(decodeHttpSandboxId(instance.sandboxId)?.localId).toBe("sbx-1");
    const createPosts = host.requests.filter(
      (r) => r.method === "POST" && r.path === "/v1/sandboxes"
    );
    expect(createPosts).toHaveLength(3);
  });

  it("fails and destroys the sandbox when creation fails on the host", async () => {
    const host = makeFakeHost({ createOutcome: "failed" });

    const exit = await runPromiseExit(
      gen(function* () {
        const session = yield* SandboxSession;
        return yield* session.create(CREATE_INPUT);
      }).pipe(provide(makeLayer(host)))
    );

    expect(exit._tag).toBe("Failure");
    expect(host.destroyed).toEqual(["sbx-1"]);
  });

  it("destroys the created sandbox when its host URL is outside the allowed set", async () => {
    const host = makeFakeHost({
      hostUrlOverride: "http://evil.example.com",
    });

    const exit = await createSessionExit(host);

    expect(exit._tag).toBe("Failure");
    expect(host.destroyed).toEqual(["sbx-1"]);
  });

  it("rejects a plain-http host URL and destroys the sandbox when insecure is not allowed", async () => {
    const host = makeFakeHost();

    const exit = await createSessionExit(host, { allowInsecureHttp: false });

    expect(exit._tag).toBe("Failure");
    expect(host.destroyed).toEqual(["sbx-1"]);
  });

  it("uploads configured files before returning", async () => {
    const host = makeFakeHost();
    const dir = mkdtempSync(join(tmpdir(), "http-sandbox-test-"));
    const localPath = join(dir, "input.json");
    writeFileSync(localPath, '{"a":1}');

    await createSession(host, {
      ...CREATE_INPUT,
      uploads: [{ localPath, remotePath: "/data/input.json", kind: "file" }],
    });

    const uploaded = host.files.get("/data/input.json");
    expect(uploaded).toBeDefined();
    expect(new TextDecoder().decode(uploaded)).toBe('{"a":1}');
  });
});

describe("makeHttpSandboxLayer instance", () => {
  it("execs with argv, env, and returns stdout/stderr/exitCode", async () => {
    const host = makeFakeHost({ execExitCode: 7 });
    const instance = await createSession(host, CREATE_INPUT);

    const result = await runPromise(
      instance.exec(["bash", "-lc", "run"], { FOO: "bar" }, 1000)
    );

    expect(result).toEqual({ stdout: "out", stderr: "err", exitCode: 7 });
    const lastExec = host.execs.at(-1);
    expect(lastExec).toEqual({
      argv: ["bash", "-lc", "run"],
      env: { FOO: "bar" },
    });
  });

  it("downloads a file from the sandbox", async () => {
    const host = makeFakeHost();
    host.files.set("/logs/out.txt", new TextEncoder().encode("artifact"));
    const instance = await createSession(host, CREATE_INPUT);
    const dir = mkdtempSync(join(tmpdir(), "http-sandbox-test-"));
    const localPath = join(dir, "out.txt");

    await runPromise(instance.downloadFile("/logs/out.txt", localPath));

    expect(readFileSync(localPath, "utf8")).toBe("artifact");
  });

  it("destroys the sandbox on the owning host", async () => {
    const host = makeFakeHost();
    const instance = await createSession(host, CREATE_INPUT);

    await runPromise(instance.destroy());

    expect(host.destroyed).toEqual(["sbx-1"]);
  });
});

describe("makeHttpSandboxLayer attach", () => {
  it("reattaches to a running sandbox by encoded id", async () => {
    const host = makeFakeHost();
    const created = await createSession(host, CREATE_INPUT);

    const attached = await runPromise(
      gen(function* () {
        const session = yield* SandboxSession;
        return yield* session.attach(created.sandboxId);
      }).pipe(provide(makeLayer(host)))
    );

    expect(attached.sandboxId).toBe(created.sandboxId);
  });

  it("fails to attach to a malformed sandbox id", async () => {
    const host = makeFakeHost();

    const exit = await runPromiseExit(
      gen(function* () {
        const session = yield* SandboxSession;
        return yield* session.attach("sb-not-an-http-id");
      }).pipe(provide(makeLayer(host)))
    );

    expect(exit._tag).toBe("Failure");
  });

  it("fails to attach when the host is outside the allowed set", async () => {
    const host = makeFakeHost();

    const exit = await runPromiseExit(
      gen(function* () {
        const session = yield* SandboxSession;
        return yield* session.attach("http://evil.example.com#sbx-1");
      }).pipe(provide(makeLayer(host)))
    );

    expect(exit._tag).toBe("Failure");
    expect(host.requests).toHaveLength(0);
  });

  it("fails to attach to a stopped sandbox", async () => {
    const host = makeFakeHost({ createOutcome: "stopped" });

    const exit = await runPromiseExit(
      gen(function* () {
        const session = yield* SandboxSession;
        return yield* session.attach(`${HOST_URL}#sbx-9`);
      }).pipe(provide(makeLayer(host)))
    );

    expect(exit._tag).toBe("Failure");
  });
});
