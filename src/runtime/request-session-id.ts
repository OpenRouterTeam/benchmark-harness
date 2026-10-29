import { createHash } from "node:crypto";

import { Reference } from "effect/Context";
import type { Effect } from "effect/Effect";
import { provideService } from "effect/Effect";

export const OPENROUTER_SESSION_ID_MAX_LENGTH = 256;

export const REQUEST_SESSION_ID_MAX_LENGTH = OPENROUTER_SESSION_ID_MAX_LENGTH;

const SAMPLE_SEGMENT_HASH_LENGTH = 12;

export const currentSampleIdRef: Reference<string | undefined> = Reference<
  string | undefined
>("@openrouter/bench-harness/runtime/currentSampleId", {
  defaultValue: () => undefined,
});

export function withCurrentSampleId(
  sampleId: string | undefined
): <A, E, R>(effect: Effect<A, E, R>) => Effect<A, E, R> {
  return (effect) => provideService(effect, currentSampleIdRef, sampleId);
}

export const getCurrentSampleId: Effect<string | undefined> =
  currentSampleIdRef;

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
