import { isRecord } from "../../src/internal/guards";

export interface CapturedRequest {
  readonly url: string;
  readonly body: Record<string, unknown>;
  readonly headers: Record<string, string>;
  readonly signal: AbortSignal | null | undefined;
}

export function installFetchSequence(
  responses: readonly Record<string, unknown>[],
  captured: CapturedRequest[]
): () => void {
  const original = globalThis.fetch;
  let responseIndex = 0;
  const stub: typeof fetch = async (input, init) => {
    const req = input instanceof Request ? input : new Request(input, init);
    const rawBody = await req.clone().text();
    captured.push({
      url: req.url,
      body: parseJsonObject(rawBody),
      headers: Object.fromEntries(req.headers.entries()),
      signal: req.signal,
    });
    const response = responses[Math.min(responseIndex++, responses.length - 1)];
    return new Response(JSON.stringify(response), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  globalThis.fetch = stub;
  return () => {
    globalThis.fetch = original;
  };
}

function parseJsonObject(raw: string): Record<string, unknown> {
  if (raw.length === 0) {
    return {};
  }
  const parsed: unknown = JSON.parse(raw);
  return isRecord(parsed) ? parsed : {};
}
