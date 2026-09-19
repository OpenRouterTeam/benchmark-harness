import { createHash } from "node:crypto";

import type { Effect } from "effect/Effect";
import type { FiberRef } from "effect/FiberRef";
import { get, set, unsafeMake } from "effect/FiberRef";

/** Mirrors the `x-session-id` bound enforced by the OpenRouter router. */
export const REQUEST_SESSION_ID_MAX_LENGTH = 256;

const SAMPLE_SEGMENT_HASH_LENGTH = 12;

export const currentSampleIdRef: FiberRef<string | undefined> = unsafeMake<
  string | undefined
>(undefined);

export function setCurrentSampleId(sampleId: string | undefined): Effect<void> {
  return set(currentSampleIdRef, sampleId);
}

export const getCurrentSampleId: Effect<string | undefined> =
  get(currentSampleIdRef);

function sanitizeSampleSegment(sampleId: string): string {
  return sampleId.replace(/[^A-Za-z0-9_-]+/g, "-");
}

/**
 * Per-task session id sent as `x-session-id` so stateful routers treat each
 * sample as a fresh task instead of a continuation of the whole run. The run
 * session id stays the dot-prefix so run-level reporting still rolls up.
 */
export function buildRequestSessionId(
  sessionId: string | undefined,
  epoch: number | undefined,
  sampleId: string | undefined
): string | undefined {
  if (sessionId === undefined) {
    return undefined;
  }
  if (epoch === undefined || sampleId === undefined) {
    return sessionId;
  }
  const prefix = `${sessionId}.${epoch}.`;
  const segment = sanitizeSampleSegment(sampleId);
  if (prefix.length + segment.length <= REQUEST_SESSION_ID_MAX_LENGTH) {
    return `${prefix}${segment}`;
  }
  const hash = createHash("sha256")
    .update(sampleId)
    .digest("hex")
    .slice(0, SAMPLE_SEGMENT_HASH_LENGTH);
  const budget =
    REQUEST_SESSION_ID_MAX_LENGTH - prefix.length - hash.length - 1;
  if (budget <= 0) {
    return sessionId;
  }
  return `${prefix}${segment.slice(0, budget)}-${hash}`;
}
