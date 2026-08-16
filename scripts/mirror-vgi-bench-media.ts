import { createHash } from "node:crypto";

import { S3Client } from "bun";

import {
  VGI_BENCH_CONFIG,
  VGI_BENCH_DATASET_PATH,
  VGI_BENCH_DEFAULT_REVISION,
  VGI_BENCH_SPLIT,
  downscaledVideoUrl,
} from "../src/benchmarks/vgi-bench/benchmark";
import { z } from "../src/internal/zod";

const HF_ROWS_BASE_URL = "https://datasets-server.huggingface.co/rows";
const HF_PAGE_SIZE = 100;

const HfRowsPageSchema = z.object({
  rows: z.array(
    z.object({
      row: z.object({
        video_id: z.string(),
        video_url: z.string(),
        question_id: z.number().int(),
      }),
    })
  ),
  num_rows_total: z.number().int(),
});

const CONTENT_TYPES: Record<string, string> = {
  mp4: "video/mp4",
  webm: "video/webm",
  mov: "video/quicktime",
  mkv: "video/x-matroska",
};

interface MirrorEnv {
  readonly endpoint: string;
  readonly bucket: string;
  readonly accessKeyId: string;
  readonly secretAccessKey: string;
  readonly publicBaseUrl: string;
  readonly keyPrefix: string;
  readonly hfToken: string | undefined;
}

interface MirrorOptions {
  readonly concurrency: number;
  readonly out: string;
  readonly force: boolean;
  readonly dryRun: boolean;
  readonly revision: string;
  readonly limit: number | undefined;
}

interface SourceCandidate {
  readonly url: string;
  readonly kind: "downscaled" | "original";
}

interface SourceVideo {
  readonly videoId: string;
  readonly originalUrl: string;
  readonly questionIds: readonly number[];
}

interface ManifestEntry {
  readonly videoId: string;
  readonly sourceUrl: string;
  readonly sourceKind: "downscaled" | "original";
  readonly key: string;
  readonly url: string;
  readonly bytes: number;
  readonly contentType: string;
  readonly sha256: string;
}

interface Manifest {
  readonly dataset: string;
  readonly config: string;
  readonly split: string;
  readonly revision: string;
  readonly publicBaseUrl: string;
  readonly generatedAt: string;
  readonly videoCount: number;
  readonly questionCount: number;
  readonly manifestHash: string;
  readonly videos: readonly ManifestEntry[];
  readonly unresolved: readonly UnresolvedEntry[];
}

interface UnresolvedEntry {
  readonly videoId: string;
  readonly attempted: readonly string[];
  readonly reason: string;
}

type MirrorOutcome =
  | { readonly kind: "mirrored"; readonly entry: ManifestEntry }
  | { readonly kind: "unresolved"; readonly reason: string };

function requireEnv(name: string): string {
  const value = process.env[name];
  if (value === undefined || value.trim() === "") {
    throw new Error(`Missing required environment variable ${name}`);
  }
  return value.trim();
}

function readEnv(): MirrorEnv {
  const publicBaseUrl = requireEnv("BENCH_MEDIA_PUBLIC_BASE_URL").replace(
    /\/+$/,
    ""
  );
  const rawPrefix = process.env["BENCH_MEDIA_KEY_PREFIX"] ?? "";
  const keyPrefix =
    rawPrefix === "" ? "" : `${rawPrefix.replaceAll(/^\/+|\/+$/g, "")}/`;
  return {
    endpoint: requireEnv("BENCH_MEDIA_S3_ENDPOINT"),
    bucket: requireEnv("BENCH_MEDIA_S3_BUCKET"),
    accessKeyId: requireEnv("BENCH_MEDIA_S3_ACCESS_KEY_ID"),
    secretAccessKey: requireEnv("BENCH_MEDIA_S3_SECRET_ACCESS_KEY"),
    publicBaseUrl,
    keyPrefix,
    hfToken: process.env["HF_TOKEN"],
  };
}

export function readOptions(argv: readonly string[]): MirrorOptions {
  const flag = (name: string): string | undefined => {
    const prefixed = `--${name}=`;
    const match = argv.find((arg) => arg.startsWith(prefixed));
    return match === undefined ? undefined : match.slice(prefixed.length);
  };
  const concurrency = Number(flag("concurrency") ?? "8");
  if (!Number.isInteger(concurrency) || concurrency < 1) {
    throw new Error("--concurrency must be a positive integer");
  }
  const rawLimit = flag("limit");
  const limit = rawLimit === undefined ? undefined : Number(rawLimit);
  if (limit !== undefined && (!Number.isInteger(limit) || limit < 1)) {
    throw new Error("--limit must be a positive integer");
  }
  return {
    concurrency,
    out: flag("out") ?? "vgi-bench-media-manifest.json",
    force: argv.includes("--force"),
    dryRun: argv.includes("--dry-run"),
    revision: flag("revision") ?? VGI_BENCH_DEFAULT_REVISION,
    limit,
  };
}

export type { ManifestEntry };

export function extensionOf(url: string): string {
  const pathname = new URL(url).pathname;
  const filename = pathname.slice(pathname.lastIndexOf("/") + 1);
  const dot = filename.lastIndexOf(".");
  if (dot === -1) {
    return "mp4";
  }
  return filename.slice(dot + 1).toLowerCase();
}

async function fetchSourceVideos(
  revision: string,
  hfToken: string | undefined
): Promise<readonly SourceVideo[]> {
  const byVideoId = new Map<
    string,
    { originalUrl: string; questionIds: number[] }
  >();
  const headers: Record<string, string> =
    hfToken === undefined ? {} : { authorization: `Bearer ${hfToken}` };
  let offset = 0;
  let total = Number.POSITIVE_INFINITY;
  while (offset < total) {
    const url = new URL(HF_ROWS_BASE_URL);
    url.searchParams.set("dataset", VGI_BENCH_DATASET_PATH);
    url.searchParams.set("config", VGI_BENCH_CONFIG);
    url.searchParams.set("split", VGI_BENCH_SPLIT);
    url.searchParams.set("revision", revision);
    url.searchParams.set("offset", String(offset));
    url.searchParams.set("length", String(HF_PAGE_SIZE));
    const response = await fetch(url, { headers });
    if (!response.ok) {
      await response.body?.cancel();
      throw new Error(
        `Hugging Face rows request failed with ${response.status}`
      );
    }
    const payload = HfRowsPageSchema.parse(await response.json());
    total = payload.num_rows_total;
    for (const { row } of payload.rows) {
      const existing = byVideoId.get(row.video_id);
      if (existing === undefined) {
        byVideoId.set(row.video_id, {
          originalUrl: row.video_url,
          questionIds: [row.question_id],
        });
      } else {
        existing.questionIds.push(row.question_id);
      }
    }
    offset += payload.rows.length;
    if (payload.rows.length === 0) {
      break;
    }
  }
  return [...byVideoId.entries()]
    .map(([videoId, value]) => ({
      videoId,
      originalUrl: value.originalUrl,
      questionIds: [...value.questionIds].sort((a, b) => a - b),
    }))
    .sort((a, b) => a.videoId.localeCompare(b.videoId));
}

export function candidateSources(
  originalUrl: string
): readonly SourceCandidate[] {
  try {
    return [
      { url: downscaledVideoUrl(originalUrl), kind: "downscaled" },
      { url: originalUrl, kind: "original" },
    ];
  } catch {
    return [{ url: originalUrl, kind: "original" }];
  }
}

async function resolveSourceUrl(
  candidates: readonly SourceCandidate[]
): Promise<SourceCandidate | undefined> {
  for (const candidate of candidates) {
    const response = await fetch(candidate.url, { method: "HEAD" });
    await response.body?.cancel();
    if (response.ok) {
      return candidate;
    }
  }
  return undefined;
}

export function describeFailure(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}

async function mirrorVideo(
  video: SourceVideo,
  candidates: readonly SourceCandidate[],
  env: MirrorEnv,
  options: MirrorOptions,
  s3: S3Client
): Promise<MirrorOutcome> {
  const source = await resolveSourceUrl(candidates);
  if (source === undefined) {
    return {
      kind: "unresolved",
      reason: "no candidate URL responded with 2xx",
    };
  }
  const extension = extensionOf(source.url);
  const contentType = CONTENT_TYPES[extension] ?? "application/octet-stream";
  const key = `${env.keyPrefix}${video.videoId}.${extension}`;
  const download = await fetch(source.url);
  if (!download.ok) {
    await download.body?.cancel();
    throw new Error(`Download of ${source.url} failed with ${download.status}`);
  }
  const bytes = new Uint8Array(await download.arrayBuffer());
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  const entry: ManifestEntry = {
    videoId: video.videoId,
    sourceUrl: source.url,
    sourceKind: source.kind,
    key,
    url: `${env.publicBaseUrl}/${key}`,
    bytes: bytes.byteLength,
    contentType,
    sha256,
  };
  if (options.dryRun) {
    return { kind: "mirrored", entry };
  }
  const target = s3.file(key);
  if (!options.force) {
    const existing = await target.stat().catch(() => undefined);
    if (existing !== undefined && existing.size === bytes.byteLength) {
      return { kind: "mirrored", entry };
    }
  }
  await target.write(bytes, { type: contentType });
  return { kind: "mirrored", entry };
}

async function mapWithConcurrency<T, R>(
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

export function hashManifest(entries: readonly ManifestEntry[]): string {
  const hasher = createHash("sha256");
  for (const entry of entries) {
    hasher.update(`${entry.videoId}\u0000${entry.url}\u0000${entry.sha256}\n`);
  }
  return hasher.digest("hex");
}

async function main(): Promise<void> {
  const env = readEnv();
  const options = readOptions(process.argv.slice(2));
  const s3 = new S3Client({
    accessKeyId: env.accessKeyId,
    secretAccessKey: env.secretAccessKey,
    bucket: env.bucket,
    endpoint: env.endpoint,
  });
  const allVideos = await fetchSourceVideos(options.revision, env.hfToken);
  const videos =
    options.limit === undefined ? allVideos : allVideos.slice(0, options.limit);
  process.stderr.write(
    `Resolved ${allVideos.length} distinct videos, mirroring ${videos.length}\n`
  );
  let done = 0;
  const outcomes = await mapWithConcurrency(
    videos,
    options.concurrency,
    async (video) => {
      const candidates = candidateSources(video.originalUrl);
      const outcome = await mirrorVideo(
        video,
        candidates,
        env,
        options,
        s3
      ).catch((cause: unknown): MirrorOutcome => ({
        kind: "unresolved",
        reason: describeFailure(cause),
      }));
      done += 1;
      process.stderr.write(
        `[${done}/${videos.length}] ${video.videoId} ${
          outcome.kind === "mirrored"
            ? outcome.entry.sourceKind
            : `UNRESOLVED ${outcome.reason}`
        }\n`
      );
      return { video, candidates, outcome };
    }
  );
  const entries = outcomes.flatMap(({ outcome }) =>
    outcome.kind === "mirrored" ? [outcome.entry] : []
  );
  const unresolved = outcomes.flatMap(({ video, candidates, outcome }) =>
    outcome.kind === "mirrored"
      ? []
      : [
          {
            videoId: video.videoId,
            attempted: candidates.map((candidate) => candidate.url),
            reason: outcome.reason,
          },
        ]
  );
  const manifest: Manifest = {
    dataset: VGI_BENCH_DATASET_PATH,
    config: VGI_BENCH_CONFIG,
    split: VGI_BENCH_SPLIT,
    revision: options.revision,
    publicBaseUrl: env.publicBaseUrl,
    generatedAt: new Date().toISOString(),
    videoCount: entries.length,
    questionCount: outcomes.reduce(
      (sum, { video, outcome }) =>
        outcome.kind === "mirrored" ? sum + video.questionIds.length : sum,
      0
    ),
    manifestHash: hashManifest(entries),
    videos: entries,
    unresolved,
  };
  await Bun.write(options.out, `${JSON.stringify(manifest, null, 2)}\n`);
  const totalBytes = entries.reduce((sum, entry) => sum + entry.bytes, 0);
  process.stderr.write(
    `Wrote ${options.out}: ${entries.length} mirrored, ${unresolved.length} unresolved, ${(totalBytes / 1e9).toFixed(2)} GB, hash ${manifest.manifestHash.slice(0, 12)}\n`
  );
  if (unresolved.length > 0) {
    process.exitCode = 1;
  }
}

if (import.meta.main) {
  await main();
}
