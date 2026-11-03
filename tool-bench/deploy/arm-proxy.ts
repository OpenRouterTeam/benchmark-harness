/**
 * Router-arm proxy for deploy/smoke-arms.sh: sits between the harness and OpenRouter (or a local
 * router), rewrites each inference request for the arm, and records per-request usage.
 *
 *   bun deploy/arm-proxy.ts --port 18787 --target https://openrouter.ai --log /tmp/x.usage.jsonl \
 *     [--universal] [--provider anthropic] [--model anthropic/claude-haiku-5.5]
 *
 * --universal adds the deferred_tools control #54394 reads (UNIVERSAL_DEFERRED_TOOLS_CONTROL in
 * packages/router/deferred-tools/admit-universal.ts). --provider pins every request to one provider
 * with no fallbacks. --model replaces the harness's model alias (Claude Code sends `claude-haiku-5-5`)
 * with the OpenRouter slug, which a local router's Chat Completions dispatch needs. Each log line carries the response usage, including
 * usage.deferred_tools_details: a universal response reports its final upstream step in the
 * native fields and the run's totals there.
 */
import { appendFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { parseArgs } from 'node:util';

const { values } = parseArgs({
  options: {
    port: { type: 'string', default: '18787' },
    target: { type: 'string', default: 'https://openrouter.ai' },
    log: { type: 'string' },
    universal: { type: 'boolean', default: false },
    provider: { type: 'string' },
    model: { type: 'string' },
  },
});

const UNIVERSAL_CONTROL = {
  protocol: 'v1',
  profile: 'universal',
  validation: 'runtime',
  search: { type: 'bm25', max_results: 8 },
};
const HOP_HEADERS = new Set(['host', 'content-length', 'accept-encoding', 'connection']);
const INFERENCE_PATH = /\/(messages|responses|chat\/completions)$/;

type Json = Record<string, unknown>;

function isJson(value: unknown): value is Json {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function rewrite(raw: Json): Json {
  const body = values.model === undefined ? raw : { ...raw, model: values.model };
  const withControl =
    values.universal && body['deferred_tools'] === undefined
      ? { ...body, deferred_tools: UNIVERSAL_CONTROL }
      : body;
  return values.provider === undefined
    ? withControl
    : { ...withControl, provider: { only: [values.provider], allow_fallbacks: false } };
}

/* Last usage object in the stream: Anthropic message_delta / message_start, or a top-level usage. */
function lastUsage(text: string): Json | undefined {
  let usage: Json | undefined;
  for (const line of text.split('\n')) {
    const data = line.startsWith('data:') ? line.slice(5).trim() : line.trim();
    if (!data.startsWith('{')) {
      continue;
    }
    try {
      const event: unknown = JSON.parse(data);
      if (!isJson(event)) {
        continue;
      }
      const message = isJson(event['message']) ? event['message'] : undefined;
      const candidate = event['usage'] ?? message?.['usage'];
      if (isJson(candidate)) {
        usage = { ...usage, ...candidate };
      }
    } catch {
      continue;
    }
  }
  return usage;
}

async function readBody(request: AsyncIterable<unknown>): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(String(chunk)));
  }
  return Buffer.concat(chunks).toString('utf8');
}

const server = createServer((request, response) => {
  void (async () => {
    const path = request.url ?? '/';
    const method = request.method ?? 'GET';
    const raw = method === 'GET' || method === 'HEAD' ? undefined : await readBody(request);
    const isInference = raw !== undefined && INFERENCE_PATH.test(new URL(path, 'http://x').pathname);
    let body = raw;
    let tools = 0;
    if (isInference) {
      const parsed: unknown = JSON.parse(raw);
      const rewritten = rewrite(isJson(parsed) ? parsed : {});
      tools = Array.isArray(rewritten['tools']) ? rewritten['tools'].length : 0;
      body = JSON.stringify(rewritten);
    }
    const headers = new Headers();
    for (let index = 0; index < request.rawHeaders.length; index += 2) {
      const key = request.rawHeaders[index] ?? '';
      const value = request.rawHeaders[index + 1];
      if (value !== undefined && !HOP_HEADERS.has(key.toLowerCase())) {
        headers.set(key, value);
      }
    }
    const started = performance.now();
    const upstream = await fetch(`${values.target}${path}`, { method, headers, body });
    const forwarded: Record<string, string> = {};
    upstream.headers.forEach((value, key) => {
      if (!['content-encoding', 'content-length', 'transfer-encoding'].includes(key)) {
        forwarded[key] = value;
      }
    });
    response.writeHead(upstream.status, forwarded);
    let text = '';
    if (upstream.body !== null) {
      const decoder = new TextDecoder();
      const reader = upstream.body.getReader();
      for (let chunk = await reader.read(); !chunk.done; chunk = await reader.read()) {
        response.write(chunk.value);
        text += decoder.decode(chunk.value, { stream: true });
      }
    }
    response.end();
    if (isInference && values.log !== undefined) {
      appendFileSync(
        values.log,
        `${JSON.stringify({
          path,
          status: upstream.status,
          latencyMs: Math.round(performance.now() - started),
          tools,
          requestBytes: body?.length ?? 0,
          usage: lastUsage(text),
          error: upstream.status >= 400 ? text.slice(0, 600) : undefined,
        })}\n`,
      );
    }
  })().catch((error: unknown) => {
    if (!response.headersSent) {
      response.writeHead(502);
    }
    response.end(`arm-proxy error: ${String(error)}`);
  });
});
server.requestTimeout = 0;
server.headersTimeout = 0;
server.listen(Number(values.port), '127.0.0.1', () => {
  process.stdout.write(
    `arm-proxy :${values.port} -> ${values.target} universal=${String(values.universal)} provider=${values.provider ?? '-'}\n`,
  );
});
