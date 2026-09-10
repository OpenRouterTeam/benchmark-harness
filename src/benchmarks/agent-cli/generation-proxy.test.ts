import { afterAll, beforeAll, describe, expect, it } from "bun:test";
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { Subprocess } from "bun";

import {
  buildGenerationProxyPrelude,
  GENERATION_ID_LINE_PREFIX,
  GENERATION_PROXY_BASE_URL,
  GENERATION_PROXY_BASE_URL_ENV,
  GENERATION_PROXY_SCRIPT,
  GENERATION_PROXY_SCRIPT_PATH,
  parseProxyGenerationIds,
} from "./generation-proxy";

const FIRST_ID = "gen-1788472540-mosiIhRdbQJ7cMvPxJTN";
const SECOND_ID = "gen-1788472541-8GGFjohh9vkZ0U4fMJyo";
const THIRD_ID = "gen-1788472726-5s5Zxbng20ez3mXkOmb1";

describe("parseProxyGenerationIds", () => {
  it("collects prefixed ids in order and drops duplicates and other lines", () => {
    const stdout = [
      '{"type":"turn.started"}',
      `${GENERATION_ID_LINE_PREFIX}${FIRST_ID}`,
      `  ${GENERATION_ID_LINE_PREFIX}${SECOND_ID}  `,
      `${GENERATION_ID_LINE_PREFIX}${FIRST_ID}`,
      GENERATION_ID_LINE_PREFIX.trim(),
      `not ${GENERATION_ID_LINE_PREFIX}${THIRD_ID}`,
    ].join("\n");
    expect(parseProxyGenerationIds(stdout)).toEqual([FIRST_ID, SECOND_ID]);
  });
});

describe("buildGenerationProxyPrelude", () => {
  it("writes the proxy, waits for its port and exports the base url", () => {
    const prelude = buildGenerationProxyPrelude("/logs/agent/codex.txt").join(
      "\n"
    );
    expect(prelude).toContain(": > /logs/agent/codex.txt");
    expect(prelude).toContain(
      `cat > ${GENERATION_PROXY_SCRIPT_PATH} <<'OR_GENERATION_PROXY_EOF'`
    );
    expect(prelude).toContain(GENERATION_PROXY_SCRIPT.trimEnd());
    expect(prelude).toContain("GEN_PROXY_UPSTREAM=https://openrouter.ai");
    expect(prelude).toContain("GEN_PROXY_LOG_PATH=/logs/agent/codex.txt");
    expect(prelude).toContain('trap \'kill "$OR_GENERATION_PROXY_PID"');
    expect(prelude).toContain("generation proxy failed to start");
    expect(prelude).toContain(
      `export ${GENERATION_PROXY_BASE_URL_ENV}="http://127.0.0.1:$(cat ${GENERATION_PROXY_SCRIPT_PATH}.$$.port)/api/v1"`
    );
    expect(GENERATION_PROXY_BASE_URL).toBe(`$${GENERATION_PROXY_BASE_URL_ENV}`);
  });
});

interface UpstreamRequest {
  readonly method: string;
  readonly path: string;
  readonly headers: Record<string, string>;
  readonly body: string;
}

describe("generation proxy script", () => {
  const dir = mkdtempSync(join(tmpdir(), "generation-proxy-test-"));
  const scriptPath = join(dir, "proxy.cjs");
  const portFile = join(dir, "proxy.port");
  const logPath = join(dir, "proxy.log");
  const upstreamRequests: UpstreamRequest[] = [];
  let upstream: ReturnType<typeof Bun.serve>;
  let proxy: Subprocess<"ignore", "pipe", "pipe">;
  let proxyBase = "";

  const UPSTREAM_ROUTES: Readonly<Record<string, () => Response>> = {
    "/api/v1/chat/completions": () =>
      Response.json(
        { id: FIRST_ID, object: "chat.completion", choices: [] },
        { headers: { "x-upstream": "yes" } }
      ),
    "/api/v1/responses": () =>
      new Response(
        [
          `data: {"type":"response.created","response":{"id":"${SECOND_ID}","object":"response"}}`,
          "",
          `data: {"type":"response.output_item.done","item":{"id":"msg_1","type":"message"}}`,
          "",
          `data: {"type":"response.completed","response":{"id":"${SECOND_ID}","object":"response"}}`,
          "",
          "data: [DONE]",
          "",
        ].join("\n"),
        { headers: { "content-type": "text/event-stream" } }
      ),
    "/api/v1/repeat": () =>
      Response.json({ id: FIRST_ID, other: { id: THIRD_ID } }),
    "/api/v1/models": () =>
      Response.json({
        data: [{ id: "openai/gpt-5-mini" }, { id: "gen-eric/model" }],
      }),
    "/api/v1/malformed": () =>
      new Response('{"id": "gen-', {
        headers: { "content-type": "application/json" },
      }),
    "/api/v1/failure": () =>
      Response.json(
        { error: { message: "rate limited", code: 429 } },
        { status: 429 }
      ),
  };

  beforeAll(async () => {
    upstream = Bun.serve({
      port: 0,
      hostname: "127.0.0.1",
      idleTimeout: 0,
      fetch: async (request) => {
        const url = new URL(request.url);
        upstreamRequests.push({
          method: request.method,
          path: url.pathname + url.search,
          headers: Object.fromEntries(request.headers.entries()),
          body: await request.text(),
        });
        const route = UPSTREAM_ROUTES[url.pathname];
        return route === undefined
          ? new Response("not found", { status: 404 })
          : route();
      },
    });
    writeFileSync(scriptPath, GENERATION_PROXY_SCRIPT);
    writeFileSync(logPath, "");
    proxy = Bun.spawn(["node", scriptPath], {
      env: {
        ...process.env,
        GEN_PROXY_PORT_FILE: portFile,
        GEN_PROXY_UPSTREAM: `http://127.0.0.1:${upstream.port}`,
        GEN_PROXY_LINE_PREFIX: GENERATION_ID_LINE_PREFIX,
        GEN_PROXY_LOG_PATH: logPath,
      },
      stdin: "ignore",
      stdout: "pipe",
      stderr: "pipe",
    });
    for (let attempt = 0; attempt < 100 && !existsSync(portFile); attempt++) {
      await Bun.sleep(50);
    }
    proxyBase = `http://127.0.0.1:${readFileSync(portFile, "utf8")}/api/v1`;
  });

  afterAll(() => {
    proxy.kill();
    upstream.stop(true);
  });

  function loggedIds(): string[] {
    return parseProxyGenerationIds(readFileSync(logPath, "utf8"));
  }

  it("forwards method, path, headers and body and relays the json response", async () => {
    const response = await fetch(`${proxyBase}/chat/completions?x=1`, {
      method: "POST",
      headers: {
        authorization: "Bearer test-key",
        "content-type": "application/json",
        "x-session-id": "sess-1",
        connection: "keep-alive",
      },
      body: JSON.stringify({ model: "openai/gpt-5-mini", messages: [] }),
    });
    expect(response.status).toBe(200);
    expect(response.headers.get("x-upstream")).toBe("yes");
    expect(await response.json()).toEqual({
      id: FIRST_ID,
      object: "chat.completion",
      choices: [],
    });
    const forwarded = upstreamRequests.at(-1);
    expect(forwarded?.method).toBe("POST");
    expect(forwarded?.path).toBe("/api/v1/chat/completions?x=1");
    expect(forwarded?.headers["authorization"]).toBe("Bearer test-key");
    expect(forwarded?.headers["x-session-id"]).toBe("sess-1");
    expect(forwarded?.headers["host"]).toBe(`127.0.0.1:${upstream.port}`);
    expect(forwarded?.body).toBe(
      JSON.stringify({ model: "openai/gpt-5-mini", messages: [] })
    );
    expect(loggedIds()).toEqual([FIRST_ID]);
  });

  it("relays sse streams and records each response id once", async () => {
    const response = await fetch(`${proxyBase}/responses`, {
      method: "POST",
      body: "{}",
    });
    expect(response.headers.get("content-type")).toBe("text/event-stream");
    const text = await response.text();
    expect(text).toContain(`"id":"${SECOND_ID}"`);
    expect(text).toContain("data: [DONE]");
    expect(loggedIds()).toEqual([FIRST_ID, SECOND_ID]);
  });

  it("records every generation id in a single response and suppresses repeats", async () => {
    await fetch(`${proxyBase}/repeat`, { method: "POST", body: "{}" });
    await fetch(`${proxyBase}/repeat`, { method: "POST", body: "{}" });
    expect(loggedIds()).toEqual([FIRST_ID, SECOND_ID, THIRD_ID]);
  });

  it("ignores model catalog ids and malformed bodies", async () => {
    await fetch(`${proxyBase}/models`);
    const malformed = await fetch(`${proxyBase}/malformed`, {
      method: "POST",
      body: "{}",
    });
    expect(await malformed.text()).toBe('{"id": "gen-');
    expect(loggedIds()).toEqual([FIRST_ID, SECOND_ID, THIRD_ID]);
  });

  it("relays upstream error statuses unchanged", async () => {
    const response = await fetch(`${proxyBase}/failure`, { method: "POST" });
    expect(response.status).toBe(429);
    expect(await response.json()).toEqual({
      error: { message: "rate limited", code: 429 },
    });
  });

  it("answers 502 when the upstream is unreachable", async () => {
    const deadPortFile = join(dir, "dead.port");
    const dead = Bun.spawn(["node", scriptPath], {
      env: {
        ...process.env,
        GEN_PROXY_PORT_FILE: deadPortFile,
        GEN_PROXY_UPSTREAM: "http://127.0.0.1:9",
        GEN_PROXY_LINE_PREFIX: GENERATION_ID_LINE_PREFIX,
        GEN_PROXY_LOG_PATH: join(dir, "dead.log"),
      },
      stdin: "ignore",
      stdout: "ignore",
      stderr: "ignore",
    });
    for (
      let attempt = 0;
      attempt < 100 && !existsSync(deadPortFile);
      attempt++
    ) {
      await Bun.sleep(50);
    }
    const response = await fetch(
      `http://127.0.0.1:${readFileSync(deadPortFile, "utf8")}/api/v1/chat/completions`,
      { method: "POST", body: "{}" }
    );
    dead.kill();
    expect(response.status).toBe(502);
    const body: unknown = await response.json();
    expect(JSON.stringify(body)).toContain("could not reach upstream");
  });

  it("echoes recorded ids on stdout with the parseable prefix", async () => {
    proxy.kill();
    const stdout = await new Response(proxy.stdout).text();
    expect(parseProxyGenerationIds(stdout)).toEqual([
      FIRST_ID,
      SECOND_ID,
      THIRD_ID,
    ]);
  });
});
