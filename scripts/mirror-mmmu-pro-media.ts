import { extname } from "node:path";
import { parseArgs } from "node:util";

import { S3Client } from "bun";

import {
  MMMU_PRO_DATASET_PATH,
  MMMU_PRO_DEFAULT_REVISION,
  MMMU_PRO_SPLIT,
  MMMU_PRO_VISION_SUBSET,
  buildMmmuProMediaManifest,
  mmmuProCachedAssetPrefix,
} from "../src/benchmarks/mmmu-pro-media-manifest";
import { z } from "../src/internal/zod";
import {
  downloadBytes,
  fetchHfRowPages,
  hashManifestEntries,
  mapWithConcurrency,
  readMediaMirrorEnv,
  sha256Hex,
  uploadUnlessPresent,
  verifyPublished,
} from "./media-mirror";

const HF_ORIGIN = "https://datasets-server.huggingface.co";
const DEFAULT_OUT = "src/benchmarks/mmmu-pro-media-manifest.json";

const RowSchema = z.object({
  id: z.string().min(1),
  image: z.object({ src: z.url() }),
});

const RevisionSchema = z.string().regex(/^[a-f0-9]{40}$/);

export interface MirrorMmmuProOptions {
  readonly revision: string;
  readonly out: string;
  readonly concurrency?: number;
  readonly force?: boolean;
  readonly dryRun?: boolean;
}

export async function mirrorMmmuProMedia(options: MirrorMmmuProOptions) {
  const revision = RevisionSchema.parse(options.revision);
  const concurrency = options.concurrency ?? 8;
  const uploadOptions = {
    force: options.force ?? false,
    dryRun: options.dryRun ?? false,
  };
  const env = readMediaMirrorEnv();
  const s3 = new S3Client(env);
  const sourcePrefix = mmmuProCachedAssetPrefix(revision);
  const images = [];
  let total = 0;
  const pages = fetchHfRowPages(
    {
      dataset: MMMU_PRO_DATASET_PATH,
      config: MMMU_PRO_VISION_SUBSET,
      split: MMMU_PRO_SPLIT,
      revision,
    },
    RowSchema
  );
  for await (const page of pages) {
    if (page.resolvedRevision !== revision) {
      throw new Error(
        `MMMU Pro rows resolved to revision ${page.resolvedRevision ?? "unknown"}, expected ${revision}`
      );
    }
    total = page.total;
    const batch = await mapWithConcurrency(
      page.rows,
      concurrency,
      async (row) => {
        const source = new URL(row.image.src);
        if (
          source.origin !== HF_ORIGIN ||
          !source.pathname.startsWith(sourcePrefix)
        ) {
          throw new Error(
            `MMMU Pro image ${row.id} does not match revision ${revision}`
          );
        }
        const bytes = await downloadBytes(source);
        if (bytes.byteLength === 0) {
          throw new Error(`MMMU Pro image ${row.id} is empty`);
        }
        const sha256 = sha256Hex(bytes);
        const contentType = Bun.file(source.pathname).type;
        const key = `${env.keyPrefix}mmmu-pro/${revision}/${sha256}${extname(source.pathname)}`;
        const outcome = await uploadUnlessPresent(
          s3,
          key,
          bytes,
          contentType,
          uploadOptions
        );
        const url = `${env.publicBaseUrl}/${key}`;
        if (outcome !== "dry-run") {
          await verifyPublished(url, { sha256, contentType });
        }
        return {
          id: row.id,
          sourcePath: source.pathname,
          url,
          bytes: bytes.byteLength,
          contentType,
          sha256,
        };
      }
    );
    images.push(...batch);
    process.stderr.write(
      `Mirrored ${images.length}/${page.total} MMMU Pro images\n`
    );
  }
  if (images.length !== total) {
    throw new Error(
      `MMMU Pro rows returned ${images.length} of ${total} expected images`
    );
  }
  images.sort((a, b) => a.id.localeCompare(b.id));
  const manifest = {
    dataset: MMMU_PRO_DATASET_PATH,
    config: MMMU_PRO_VISION_SUBSET,
    split: MMMU_PRO_SPLIT,
    revision,
    manifestHash: hashManifestEntries(images),
    images,
  };
  buildMmmuProMediaManifest(manifest);
  if (!uploadOptions.dryRun) {
    await Bun.write(options.out, `${JSON.stringify(manifest, null, 2)}\n`);
  }
  return manifest;
}

if (import.meta.main) {
  const { values } = parseArgs({
    args: Bun.argv.slice(2),
    options: {
      revision: { type: "string", default: MMMU_PRO_DEFAULT_REVISION },
      out: { type: "string", default: DEFAULT_OUT },
      concurrency: { type: "string", default: "8" },
      force: { type: "boolean", default: false },
      "dry-run": { type: "boolean", default: false },
    },
  });
  const concurrency = Number(values.concurrency);
  if (!Number.isInteger(concurrency) || concurrency < 1) {
    throw new Error("--concurrency must be a positive integer");
  }
  await mirrorMmmuProMedia({
    revision: values.revision,
    out: values.out,
    concurrency,
    force: values.force,
    dryRun: values["dry-run"],
  });
}
