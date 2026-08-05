export function blockNetwork(): () => void {
  const original = globalThis.fetch;
  const stub: typeof fetch = (input) => {
    const url = input instanceof Request ? input.url : String(input);
    return Promise.reject(new Error(`network disabled in test: ${url}`));
  };
  globalThis.fetch = stub;
  return () => {
    globalThis.fetch = original;
  };
}
