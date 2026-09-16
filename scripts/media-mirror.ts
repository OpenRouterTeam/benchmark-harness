import { createHash } from "node:crypto";

import type { S3Client } from "bun";

import { z } from "../src/internal/zod";

const HF_ROWS_BASE_URL = "https://datasets-server.huggingface.co/rows";
const HF_PAGE_SIZE = 100;
const FETCH_ATTEMPTS = 4;
const FETCH_BACKOFF_MS = 1_000;

function isTransientStatus(status: number): boolean {
  return status === 429 || status >= 500;
}

async function fetchWithRetry(
  url: string | URL,
  init: RequestInit,
  describe: string
): Promise<Response> {
  for (let attempt = 1; ; attempt += 1) {
    const response = await fetch(url, init);
    if (response.ok) {
      return response;
    }
    await response.body?.cancel();
    if (attempt >= FETCH_ATTEMPTS || !isTransientStatus(response.status)) {
      throw new Error(`${describe} failed with ${response.status}`);
    }
    await Bun.sleep(FETCH_BACKOFF_MS * 2 ** (attempt - 1));
  }
}

function requireEnv(name: string): string {
  const value = process.env[name]?.trim();
  if (!value) {
    throw new Error(`Missing required environment variable ${name}`);
  }
  return value;
}

export function normalizeKeyPrefix(rawPrefix: string | undefined): string {
  const stripped = (rawPrefix ?? "").trim().replaceAll(/^\/+|\/+$/g, "");
  return stripped === "" ? "" : `${stripped}/`;
}

export function readMediaMirrorEnv() {
  return {
    endpoint: requireEnv("BENCH_MEDIA_S3_ENDPOINT"),
    bucket: requireEnv("BENCH_MEDIA_S3_BUCKET"),
    accessKeyId: requireEnv("BENCH_MEDIA_S3_ACCESS_KEY_ID"),
    secretAccessKey: requireEnv("BENCH_MEDIA_S3_SECRET_ACCESS_KEY"),
    publicBaseUrl: requireEnv("BENCH_MEDIA_PUBLIC_BASE_URL").replace(
      /\/+$/,
      ""
    ),
    keyPrefix: normalizeKeyPrefix(process.env["BENCH_MEDIA_KEY_PREFIX"]),
  };
}

export type MediaMirrorEnv = ReturnType<typeof readMediaMirrorEnv>;

export function sha256Hex(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

export function describeFailure(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

export interface HfRowsSource {
  readonly dataset: string;
  readonly config: string;
  readonly split: string;
  readonly revision: string;
  readonly hfToken?: string | undefined;
}

export interface HfRowsPage<Row> {
  readonly rows: readonly Row[];
  readonly offset: number;
  readonly total: number;
  readonly resolvedRevision: string | null;
}

export async function* fetchHfRowPages<Row>(
  source: HfRowsSource,
  rowSchema: z.ZodType<Row>
): AsyncGenerator<HfRowsPage<Row>> {
  const pageSchema = z.object({
    rows: z.array(z.object({ row: rowSchema })),
    num_rows_total: z.number().int(),
  });
  const headers: Record<string, string> =
    source.hfToken === undefined
      ? {}
      : { authorization: `Bearer ${source.hfToken}` };
  let offset = 0;
  let total = Number.POSITIVE_INFINITY;
  while (offset < total) {
    const url = new URL(HF_ROWS_BASE_URL);
    url.searchParams.set("dataset", source.dataset);
    url.searchParams.set("config", source.config);
    url.searchParams.set("split", source.split);
    url.searchParams.set("revision", source.revision);
    url.searchParams.set("offset", String(offset));
    url.searchParams.set("length", String(HF_PAGE_SIZE));
    const response = await fetchWithRetry(
      url,
      { headers },
      `Hugging Face rows request for ${source.dataset} at offset ${offset}`
    );
    const page = pageSchema.parse(await response.json());
    total = page.num_rows_total;
    yield {
      rows: page.rows.map((entry) => entry.row),
      offset,
      total,
      resolvedRevision: response.headers.get("x-revision"),
    };
    if (page.rows.length === 0) {
      break;
    }
    offset += page.rows.length;
  }
}

export async function downloadBytes(url: string | URL): Promise<Uint8Array> {
  const response = await fetchWithRetry(url, {}, `Download of ${String(url)}`);
  return new Uint8Array(await response.arrayBuffer());
}

export interface UploadOptions {
  readonly force: boolean;
  readonly dryRun: boolean;
}

export type UploadOutcome = "uploaded" | "skipped" | "dry-run";

export async function uploadUnlessPresent(
  s3: S3Client,
  key: string,
  bytes: Uint8Array,
  contentType: string,
  options: UploadOptions
): Promise<UploadOutcome> {
  if (options.dryRun) {
    return "dry-run";
  }
  const target = s3.file(key);
  if (!options.force) {
    const existing = await target.stat().catch(() => undefined);
    if (existing !== undefined && existing.size === bytes.byteLength) {
      return "skipped";
    }
  }
  await target.write(bytes, { type: contentType });
  return "uploaded";
}

export async function verifyPublished(
  url: string,
  expected: { readonly sha256: string; readonly contentType: string }
): Promise<void> {
  const response = await fetch(url);
  if (!response.ok) {
    await response.body?.cancel();
    throw new Error(`Published ${url} returned HTTP ${response.status}`);
  }
  const contentType = response.headers.get("content-type")?.split(";")[0];
  const sha256 = sha256Hex(new Uint8Array(await response.arrayBuffer()));
  if (contentType !== expected.contentType || sha256 !== expected.sha256) {
    throw new Error(`Published ${url} failed public readback verification`);
  }
}

export async function mapWithConcurrency<T, R>(
  items: readonly T[],
  concurrency: number,
  worker: (item: T, index: number) => Promise<R>
): Promise<readonly R[]> {
  const results = new Array<R>(items.length);
  let cursor = 0;
  const runners = Array.from(
    { length: Math.min(concurrency, items.length) },
    async () => {
      while (true) {
        const index = cursor;
        cursor += 1;
        if (index >= items.length) {
          return;
        }
        results[index] = await worker(items[index]!, index);
      }
    }
  );
  await Promise.all(runners);
  return results;
}

export function hashManifestEntries(
  entries: readonly {
    readonly id: string;
    readonly url: string;
    readonly sha256: string;
  }[]
): string {
  const hasher = createHash("sha256");
  for (const entry of entries) {
    hasher.update(`${entry.id}\u0000${entry.url}\u0000${entry.sha256}\n`);
  }
  return hasher.digest("hex");
}
