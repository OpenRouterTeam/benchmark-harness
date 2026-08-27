export const ALLOWED_TRACE_HEADER_NAMES = [
  "traceparent",
  "tracestate",
  "x-or-traceparent",
  "x-benchmark-trace",
] as const;

export type AllowedTraceHeaderName =
  (typeof ALLOWED_TRACE_HEADER_NAMES)[number];

const ALLOWED_TRACE_HEADER_SET: ReadonlySet<string> = new Set(
  ALLOWED_TRACE_HEADER_NAMES
);

export function filterTraceHeaders(
  headers: Readonly<Record<string, string>> | undefined
): Record<string, string> | undefined {
  if (headers === undefined) {
    return undefined;
  }
  const filtered = Object.fromEntries(
    Object.entries(headers)
      .map(([name, value]): readonly [string, string] => [
        name.toLowerCase(),
        value,
      ])
      .filter(([name]) => ALLOWED_TRACE_HEADER_SET.has(name))
  );
  return Object.keys(filtered).length > 0 ? filtered : undefined;
}
