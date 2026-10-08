import { createHash } from "node:crypto";

import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { as, forEach, retry, tryPromise } from "effect/Effect";
import { exponential, intersect, recurs } from "effect/Schedule";

import type { RunResult } from "../harness/run";
import type { RetryConfig } from "../runtime/retry";

export interface InlineArtifact {
  readonly path: string;
  readonly contentType: string;
  readonly bytes: Uint8Array;
}

export interface ArtifactSink {
  readonly uriFor: (path: string) => string;
  readonly put: (artifact: InlineArtifact) => Promise<void>;
}

export class ArtifactUploadError extends TaggedError("ArtifactUploadError")<{
  readonly message: string;
}> {}

const DATA_URL = /^data:([\w.+-]+\/[\w.+-]+)(?:;[^,;]*)*;base64,(.*)$/s;
const BASE64 = /^[A-Za-z0-9+/]+={0,2}$/;

const EXTENSIONS: Readonly<Record<string, string>> = {
  "application/octet-stream": "bin",
  "image/jpeg": "jpg",
  "image/svg+xml": "svg",
  "audio/mpeg": "mp3",
  "audio/x-wav": "wav",
};

const UPLOAD_CONCURRENCY = 8;

export function offloadInlineBase64(
  result: RunResult,
  sink: ArtifactSink,
  retryConfig: RetryConfig = {}
): Effect<RunResult, ArtifactUploadError> {
  const artifactsByPath = new Map<string, InlineArtifact>();
  const sampleScores = replaceBase64(
    result.sampleScores,
    (contentType, base64) => {
      const artifact = toArtifact(contentType, base64);
      artifactsByPath.set(artifact.path, artifact);
      return sink.uriFor(artifact.path);
    }
  ) as RunResult["sampleScores"];
  const schedule = exponential(retryConfig.baseDelayMs ?? 500).pipe(
    intersect(recurs(retryConfig.maxRetries ?? 4))
  );
  return forEach(
    artifactsByPath.values(),
    (artifact) =>
      tryPromise({
        try: () => sink.put(artifact),
        catch: (cause) =>
          new ArtifactUploadError({
            message: `Failed to upload ${artifact.path}: ${String(cause)}`,
          }),
      }).pipe(retry(schedule)),
    { concurrency: UPLOAD_CONCURRENCY, discard: true }
  ).pipe(as({ ...result, sampleScores }));
}

type ReplacePayload = (contentType: string, base64: string) => string;

function replaceBase64(
  value: unknown,
  replacePayload: ReplacePayload
): unknown {
  if (typeof value === "string") {
    const match = DATA_URL.exec(value);
    return match?.[1] !== undefined &&
      match[2] !== undefined &&
      BASE64.test(match[2])
      ? replacePayload(match[1], match[2])
      : value;
  }
  if (Array.isArray(value)) {
    return value.map((item) => replaceBase64(item, replacePayload));
  }
  if (!isPlainObject(value)) {
    return value;
  }
  return Object.fromEntries(
    Object.entries(value).map(([key, field]) => {
      const contentType = bareBase64ContentType(value, key);
      return [
        key,
        contentType !== undefined &&
        typeof field === "string" &&
        !field.startsWith("data:") &&
        BASE64.test(field)
          ? replacePayload(contentType, field)
          : replaceBase64(field, replacePayload),
      ];
    })
  );
}

function bareBase64ContentType(
  record: Readonly<Record<string, unknown>>,
  key: string
): string | undefined {
  if (key === "file_data") {
    return "application/octet-stream";
  }
  if (key !== "data") {
    return undefined;
  }
  if (typeof record.mimeType === "string") {
    return record.mimeType;
  }
  if (record.type === "base64" && typeof record.media_type === "string") {
    return record.media_type;
  }
  return typeof record.format === "string"
    ? `audio/${record.format}`
    : undefined;
}

function toArtifact(contentType: string, base64: string): InlineArtifact {
  const bytes = Buffer.from(base64, "base64");
  const hex = createHash("sha256").update(bytes).digest("hex");
  return {
    path: `artifacts/sha256/${hex}.${extensionFor(contentType)}`,
    contentType,
    bytes,
  };
}

function extensionFor(contentType: string): string {
  const normalized = contentType.toLowerCase();
  const subtype = normalized.split("/")[1]?.replaceAll(/[^a-z0-9]/g, "") ?? "";
  return EXTENSIONS[normalized] ?? (subtype === "" ? "bin" : subtype);
}

function isPlainObject(
  value: unknown
): value is Readonly<Record<string, unknown>> {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const proto: unknown = Object.getPrototypeOf(value);
  return proto === Object.prototype || proto === null;
}
