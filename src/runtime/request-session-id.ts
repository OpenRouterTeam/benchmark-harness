import { createHash } from "node:crypto";

import type { Effect } from "effect/Effect";
import type { FiberRef } from "effect/FiberRef";
import { get, set, unsafeMake } from "effect/FiberRef";

export const OPENROUTER_SESSION_ID_MAX_LENGTH = 256;

export const REQUEST_SESSION_ID_MAX_LENGTH = OPENROUTER_SESSION_ID_MAX_LENGTH;

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
  return sampleId.replaceAll(/[^A-Za-z0-9_-]+/g, "-");
}

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
