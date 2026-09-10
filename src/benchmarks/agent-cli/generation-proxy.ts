export const GENERATION_PROXY_SCRIPT_PATH =
  "/tmp/openrouter-generation-proxy.cjs" as const;

const GENERATION_PROXY_PORT_FILE = `${GENERATION_PROXY_SCRIPT_PATH}.$$.port`;

export const GENERATION_PROXY_UPSTREAM = "https://openrouter.ai" as const;

export const GENERATION_PROXY_BASE_URL_ENV =
  "OR_GENERATION_PROXY_BASE_URL" as const;

export const GENERATION_PROXY_BASE_URL =
  `$${GENERATION_PROXY_BASE_URL_ENV}` as const;

export const GENERATION_ID_LINE_PREFIX = "OR_GENERATION_ID " as const;

const GENERATION_ID_PATTERN = /"id"\s*:\s*"(gen-\d+-[A-Za-z0-9_-]+)"/g;

const PROXY_READY_ATTEMPTS = 50;

export const GENERATION_PROXY_SCRIPT = String.raw`"use strict";
const http = require("node:http");
const fs = require("node:fs");
const portFile = process.env.GEN_PROXY_PORT_FILE;
const upstream = new URL(process.env.GEN_PROXY_UPSTREAM);
const prefix = process.env.GEN_PROXY_LINE_PREFIX;
const logPath = process.env.GEN_PROXY_LOG_PATH;
const idPattern = ${GENERATION_ID_PATTERN.toString()};
const hopHeaders = new Set(["host", "connection", "content-length", "transfer-encoding", "content-encoding"]);
const seen = new Set();
function record(id) {
  if (seen.has(id)) return;
  seen.add(id);
  process.stdout.write(prefix + id + "\n");
  fs.appendFileSync(logPath, prefix + id + "\n");
}
function scan(text) {
  for (const match of text.matchAll(idPattern)) record(match[1]);
}
async function readBody(req) {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  return Buffer.concat(chunks);
}
async function handle(req, res) {
  const body = await readBody(req);
  const headers = {};
  for (const [name, value] of Object.entries(req.headers)) {
    if (!hopHeaders.has(name) && typeof value === "string") headers[name] = value;
  }
  headers.host = upstream.host;
  let upstreamRes;
  try {
    upstreamRes = await fetch(new URL(req.url, upstream), {
      method: req.method,
      headers,
      body: body.length > 0 ? body : undefined,
      redirect: "manual",
    });
  } catch (error) {
    res.writeHead(502, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: { message: "generation proxy could not reach upstream: " + String(error) } }));
    return;
  }
  const responseHeaders = {};
  upstreamRes.headers.forEach((value, name) => {
    if (!hopHeaders.has(name)) responseHeaders[name] = value;
  });
  res.writeHead(upstreamRes.status, responseHeaders);
  if (upstreamRes.body === null) {
    res.end();
    return;
  }
  const decoder = new TextDecoder();
  const inspect = req.method !== "GET";
  let pending = "";
  for await (const chunk of upstreamRes.body) {
    res.write(chunk);
    if (!inspect) continue;
    pending += decoder.decode(chunk, { stream: true });
    const cut = pending.lastIndexOf("\n");
    if (cut >= 0) {
      scan(pending.slice(0, cut + 1));
      pending = pending.slice(cut + 1);
    }
  }
  scan(pending + decoder.decode());
  res.end();
}
const server = http.createServer((req, res) => {
  handle(req, res).catch((error) => {
    process.stderr.write("generation proxy request failed: " + String(error) + "\n");
    if (!res.headersSent) res.writeHead(502);
    res.end();
  });
});
server.keepAliveTimeout = 0;
server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(portFile + ".tmp", String(server.address().port));
  fs.renameSync(portFile + ".tmp", portFile);
});
`;

export function buildGenerationProxyPrelude(logPath: string): string[] {
  return [
    `: > ${logPath}`,
    `cat > ${GENERATION_PROXY_SCRIPT_PATH} <<'OR_GENERATION_PROXY_EOF'`,
    GENERATION_PROXY_SCRIPT.trimEnd(),
    "OR_GENERATION_PROXY_EOF",
    `rm -f ${GENERATION_PROXY_PORT_FILE}`,
    `GEN_PROXY_PORT_FILE=${GENERATION_PROXY_PORT_FILE} GEN_PROXY_UPSTREAM=${GENERATION_PROXY_UPSTREAM} GEN_PROXY_LINE_PREFIX=${JSON.stringify(GENERATION_ID_LINE_PREFIX)} GEN_PROXY_LOG_PATH=${logPath} node ${GENERATION_PROXY_SCRIPT_PATH} 2>>/tmp/openrouter-generation-proxy.err &`,
    "OR_GENERATION_PROXY_PID=$!",
    `trap 'kill "$OR_GENERATION_PROXY_PID" 2>/dev/null || true; rm -f ${GENERATION_PROXY_PORT_FILE}' EXIT`,
    `for _ in $(seq ${PROXY_READY_ATTEMPTS}); do [ -s ${GENERATION_PROXY_PORT_FILE} ] && break; sleep 0.2; done`,
    `[ -s ${GENERATION_PROXY_PORT_FILE} ] || { echo "generation proxy failed to start" >&2; exit 3; }`,
    `export ${GENERATION_PROXY_BASE_URL_ENV}="http://127.0.0.1:$(cat ${GENERATION_PROXY_PORT_FILE})/api/v1"`,
  ];
}

export function parseProxyGenerationIds(stdout: string): string[] {
  const ids: string[] = [];
  for (const line of stdout.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed.startsWith(GENERATION_ID_LINE_PREFIX)) {
      continue;
    }
    const id = trimmed.slice(GENERATION_ID_LINE_PREFIX.length).trim();
    if (id.length > 0 && !ids.includes(id)) {
      ids.push(id);
    }
  }
  return ids;
}
