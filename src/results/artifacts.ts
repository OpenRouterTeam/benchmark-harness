import { createHash } from "node:crypto";

import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { forEach, map, retry, tryPromise } from "effect/Effect";
import { exponential, intersect, recurs } from "effect/Schedule";

import type { RunResult } from "../harness/run";
import type { RetryConfig } from "../runtime/retry";

export interface InlineArtifact {
  readonly path: string;
  readonly contentType: string;
  readonly bytes: Uint8Array;
}

export interface ArtifactSink {
  readonly put: (artifact: InlineArtifact) => Promise<string>;
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
  const found = new PayloadMap<InlineArtifact>();
  visit(result.sampleScores, (payload, contentType, base64) => {
    if (found.get(contentType, payload) === undefined) {
      found.set(contentType, payload, toArtifact(contentType, base64));
    }
    return payload;
  });
  const uniqueByPath = new Map(
    found.values().map((artifact) => [artifact.path, artifact])
  );
  const schedule = exponential(retryConfig.baseDelayMs ?? 500).pipe(
    intersect(recurs(retryConfig.maxRetries ?? 4))
  );
  return forEach(
    uniqueByPath.values(),
    (artifact) =>
      tryPromise({
        try: () => sink.put(artifact),
        catch: (cause) =>
          new ArtifactUploadError({
            message: `Failed to upload ${artifact.path}: ${String(cause)}`,
          }),
      }).pipe(
        retry(schedule),
        map((uri) => [artifact.path, uri] as const)
      ),
    { concurrency: UPLOAD_CONCURRENCY }
  ).pipe(
    map((uploaded) => {
      const uriByPath = new Map(uploaded);
      return {
        ...result,
        sampleScores: visit(result.sampleScores, (payload, contentType) => {
          const artifact = found.get(contentType, payload);
          return (artifact && uriByPath.get(artifact.path)) ?? payload;
        }) as RunResult["sampleScores"],
      };
    })
  );
}

class PayloadMap<V> {
  private readonly byContentType = new Map<string, Map<string, V>>();

  get(contentType: string, payload: string): V | undefined {
    return this.byContentType.get(contentType)?.get(payload);
  }

  set(contentType: string, payload: string, value: V): void {
    const byPayload = this.byContentType.get(contentType) ?? new Map();
    byPayload.set(payload, value);
    this.byContentType.set(contentType, byPayload);
  }

  values(): V[] {
    return [...this.byContentType.values()].flatMap((byPayload) => [
      ...byPayload.values(),
    ]);
  }
}

type OnPayload = (
  payload: string,
  contentType: string,
  base64: string
) => string;

function visit(value: unknown, onPayload: OnPayload): unknown {
  if (typeof value === "string") {
    const match = DATA_URL.exec(value);
    return match?.[1] !== undefined &&
      match[2] !== undefined &&
      BASE64.test(match[2])
      ? onPayload(value, match[1], match[2])
      : value;
  }
  if (Array.isArray(value)) {
    return value.map((item) => visit(item, onPayload));
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
          ? onPayload(field, contentType, field)
          : visit(field, onPayload),
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
