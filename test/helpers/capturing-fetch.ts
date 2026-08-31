export interface CapturingFetch {
  readonly headers: () => Headers | undefined;
  readonly restore: () => void;
}

export function installCapturingFetch(
  body: string,
  contentType: string
): CapturingFetch {
  const original = globalThis.fetch;
  let captured: Headers | undefined;
  const stub: typeof fetch = async (input, init) => {
    const request = input instanceof Request ? input : new Request(input, init);
    if (captured === undefined) {
      captured = request.headers;
    }
    return new Response(body, {
      status: 200,
      headers: { "content-type": contentType },
    });
  };
  globalThis.fetch = stub;
  return {
    headers: () => captured,
    restore: () => {
      globalThis.fetch = original;
    },
  };
}
