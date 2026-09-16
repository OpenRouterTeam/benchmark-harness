import { createHash } from "node:crypto";
import { mkdir } from "node:fs/promises";
import { extname, join } from "node:path";
import { parseArgs } from "node:util";

import type { MmmuProMediaManifest } from "../src/benchmarks/mmmu-pro-media-manifest";
import {
  buildMmmuProMediaManifest,
  hashMmmuProMedia,
  MmmuProMediaManifestSchema,
} from "../src/benchmarks/mmmu-pro-media-manifest";
import { z } from "../src/internal/zod";
import {
  createMediaMirrorClient,
  readMediaMirrorEnv,
  uploadMedia,
} from "./media-mirror";

const RowsSchema = z.object({
  rows: z.array(
    z.object({
      row: z.object({
        id: z.string().min(1),
        image: z.object({ src: z.url() }),
      }),
    })
  ),
  num_rows_total: z.number().int().positive(),
});

export async function prepareMmmuProMedia(options: {
  directory: string;
  revision: string;
  publicBaseUrl: string;
}): Promise<MmmuProMediaManifest> {
  const revision = z
    .string()
    .regex(/^[a-f0-9]{40}$/)
    .parse(options.revision);
  const publicBaseUrl = z
    .url()
    .parse(options.publicBaseUrl)
    .replace(/\/+$/, "");
  const images: MmmuProMediaManifest["images"] = [];
  await mkdir(options.directory, { recursive: true });
  let total = Number.POSITIVE_INFINITY;
  for (let offset = 0; offset < total; offset += 100) {
    const response = await fetch(
      `https://datasets-server.huggingface.co/rows?dataset=MMMU/MMMU_Pro&config=vision&split=test&offset=${offset}&length=100`
    );
    if (!response.ok || response.headers.get("x-revision") !== revision) {
      await response.body?.cancel();
      throw new Error(
        `MMMU Pro rows must return 200 at revision ${revision} (HTTP ${response.status})`
      );
    }
    const page = RowsSchema.parse(await response.json());
    total = page.num_rows_total;
    if (page.rows.length !== Math.min(100, total - offset)) {
      throw new Error(`Incomplete MMMU Pro page at offset ${offset}`);
    }
    for (let start = 0; start < page.rows.length; start += 8) {
      const batch = await Promise.all(
        page.rows.slice(start, start + 8).map(async ({ row }) => {
          const source = new URL(row.image.src);
          if (
            source.origin !== "https://datasets-server.huggingface.co" ||
            !source.pathname.startsWith(
              `/cached-assets/MMMU/MMMU_Pro/--/${revision}/--/vision/test/`
            )
          ) {
            throw new Error(
              `MMMU Pro image ${row.id} does not match revision ${revision}`
            );
          }
          const download = await fetch(source);
          if (!download.ok) {
            await download.body?.cancel();
            throw new Error(
              `MMMU Pro image ${row.id}: HTTP ${download.status}`
            );
          }
          const bytes = new Uint8Array(await download.arrayBuffer());
          const sha256 = createHash("sha256").update(bytes).digest("hex");
          const filename = `${sha256}${extname(source.pathname)}`;
          await Bun.write(join(options.directory, filename), bytes);
          return {
            id: row.id,
            sourcePath: source.pathname,
            url: `${publicBaseUrl}/mmmu-pro/${revision}/${filename}`,
            bytes: bytes.byteLength,
            contentType: Bun.file(source.pathname).type,
            sha256,
          };
        })
      );
      images.push(...batch);
    }
    process.stderr.write(
      `Prepared ${images.length}/${total} MMMU Pro images\n`
    );
  }
  images.sort((a, b) => a.id.localeCompare(b.id));
  const manifest: MmmuProMediaManifest = {
    dataset: "MMMU/MMMU_Pro",
    config: "vision",
    split: "test",
    revision,
    manifestHash: hashMmmuProMedia(images),
    images,
  };
  buildMmmuProMediaManifest(manifest);
  await Bun.write(
    join(options.directory, "manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`
  );
  return manifest;
}

async function publishMmmuProMedia(
  directory: string,
  out: string
): Promise<void> {
  const manifest = MmmuProMediaManifestSchema.parse(
    await Bun.file(join(directory, "manifest.json")).json()
  );
  buildMmmuProMediaManifest(manifest);
  const env = readMediaMirrorEnv();
  const s3 = createMediaMirrorClient(env);
  for (const image of manifest.images) {
    const filename = `${image.sha256}${extname(new URL(image.url).pathname)}`;
    const bytes = new Uint8Array(
      await Bun.file(join(directory, filename)).arrayBuffer()
    );
    if (
      bytes.byteLength !== image.bytes ||
      createHash("sha256").update(bytes).digest("hex") !== image.sha256
    ) {
      throw new Error(
        `Prepared MMMU Pro image ${image.id} failed checksum verification`
      );
    }
    const key = `${env.keyPrefix}mmmu-pro/${manifest.revision}/${filename}`;
    await uploadMedia(s3, key, bytes, image.contentType);
    image.url = `${env.publicBaseUrl}/${key}`;
    const check = await fetch(image.url);
    if (!check.ok) {
      await check.body?.cancel();
      throw new Error(
        `Published MMMU Pro image ${image.id}: HTTP ${check.status}`
      );
    }
    if (
      check.headers.get("content-type")?.split(";")[0] !== image.contentType ||
      createHash("sha256")
        .update(new Uint8Array(await check.arrayBuffer()))
        .digest("hex") !== image.sha256
    ) {
      throw new Error(
        `Published MMMU Pro image ${image.id} failed public readback verification`
      );
    }
  }
  manifest.manifestHash = hashMmmuProMedia(manifest.images);
  await Bun.write(out, `${JSON.stringify(manifest, null, 2)}\n`);
}

if (import.meta.main) {
  const { values } = parseArgs({
    args: Bun.argv.slice(2),
    options: {
      prepare: { type: "boolean", default: false },
      directory: { type: "string" },
      revision: { type: "string" },
      "public-base-url": { type: "string" },
      out: {
        type: "string",
        default: "src/benchmarks/mmmu-pro-media-manifest.json",
      },
    },
  });
  if (!values.directory) {
    throw new Error("--directory is required for prepared image files");
  }
  if (values.prepare) {
    await prepareMmmuProMedia({
      directory: values.directory,
      revision: values.revision ?? "",
      publicBaseUrl: values["public-base-url"] ?? "",
    });
  } else {
    await publishMmmuProMedia(values.directory, values.out);
  }
}
