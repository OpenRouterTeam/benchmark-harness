import { S3Client } from "bun";

import {
  VGI_BENCH_CONFIG,
  VGI_BENCH_DATASET_PATH,
  VGI_BENCH_DEFAULT_REVISION,
  VGI_BENCH_SPLIT,
  downscaledVideoUrl,
} from "../src/benchmarks/vgi-bench/benchmark";
import { z } from "../src/internal/zod";
import type { MediaMirrorEnv } from "./media-mirror";
import {
  describeFailure,
  downloadBytes,
  fetchHfRowPages,
  hashManifestEntries,
  mapWithConcurrency,
  readMediaMirrorEnv,
  sha256Hex,
  uploadUnlessPresent,
} from "./media-mirror";

const VgiRowSchema = z.object({
  video_id: z.string(),
  video_url: z.string(),
  question_id: z.number().int(),
});

const CONTENT_TYPES: Record<string, string> = {
  mp4: "video/mp4",
  webm: "video/webm",
  mov: "video/quicktime",
  mkv: "video/x-matroska",
};

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
  const pages = fetchHfRowPages(
    {
      dataset: VGI_BENCH_DATASET_PATH,
      config: VGI_BENCH_CONFIG,
      split: VGI_BENCH_SPLIT,
      revision,
      hfToken,
    },
    VgiRowSchema
  );
  for await (const page of pages) {
    for (const row of page.rows) {
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
    const response = await fetch(candidate.url, { method: "HEAD" }).catch(
      () => undefined
    );
    await response?.body?.cancel();
    if (response?.ok === true) {
      return candidate;
    }
  }
  return undefined;
}

async function mirrorVideo(
  video: SourceVideo,
  candidates: readonly SourceCandidate[],
  env: MediaMirrorEnv,
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
  const bytes = await downloadBytes(source.url);
  const sha256 = sha256Hex(bytes);
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
  await uploadUnlessPresent(s3, key, bytes, contentType, options);
  return { kind: "mirrored", entry };
}

export function hashManifest(entries: readonly ManifestEntry[]): string {
  return hashManifestEntries(
    entries.map((entry) => ({
      id: entry.videoId,
      url: entry.url,
      sha256: entry.sha256,
    }))
  );
}

async function main(): Promise<void> {
  const env = readMediaMirrorEnv();
  const options = readOptions(process.argv.slice(2));
  const s3 = new S3Client(env);
  const allVideos = await fetchSourceVideos(
    options.revision,
    process.env["HF_TOKEN"]
  );
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
  const mirroredQuestionIds = outcomes.flatMap(({ video, outcome }) =>
    outcome.kind === "mirrored" ? video.questionIds : []
  );
  const manifest: Manifest = {
    dataset: VGI_BENCH_DATASET_PATH,
    config: VGI_BENCH_CONFIG,
    split: VGI_BENCH_SPLIT,
    revision: options.revision,
    publicBaseUrl: env.publicBaseUrl,
    generatedAt: new Date().toISOString(),
    videoCount: entries.length,
    questionCount: mirroredQuestionIds.length,
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
