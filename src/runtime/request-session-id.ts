import { createHash } from "node:crypto";

import type { Effect } from "effect/Effect";
import type { FiberRef } from "effect/FiberRef";
import { get, set, unsafeMake } from "effect/FiberRef";

export const OPENROUTER_SESSION_ID_MAX_LENGTH = 256;

export const REQUEST_SESSION_ID_MAX_LENGTH = OPENROUTER_SESSION_ID_MAX_LENGTH;

const SAMPLE_SEGMENT_HASH_LENGTH = 12;

const FULL_IDENTITY_HASH_LENGTH = 32;

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

function hashHex(value: string, length: number): string {
  return createHash("sha256").update(value).digest("hex").slice(0, length);
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
  const sanitized = sanitizeSampleSegment(sampleId);
  const hash = hashHex(sampleId, SAMPLE_SEGMENT_HASH_LENGTH);
  const segment = sanitized === sampleId ? sanitized : `${sanitized}-${hash}`;
  if (prefix.length + segment.length <= REQUEST_SESSION_ID_MAX_LENGTH) {
    return `${prefix}${segment}`;
  }
  const budget =
    REQUEST_SESSION_ID_MAX_LENGTH - prefix.length - hash.length - 1;
  if (budget > 0) {
    return `${prefix}${sanitized.slice(0, budget)}-${hash}`;
  }
  const identityHash = hashHex(
    `${sessionId}.${epoch}.${sampleId}`,
    FULL_IDENTITY_HASH_LENGTH
  );
  const head = sessionId.slice(
    0,
    REQUEST_SESSION_ID_MAX_LENGTH - identityHash.length - 1
  );
  return `${head}.${identityHash}`;
}
