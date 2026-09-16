import { createHash } from "node:crypto";
import { extname } from "node:path";
import { parseArgs } from "node:util";

import { S3Client } from "bun";

import { buildMmmuProMediaManifest } from "../src/benchmarks/mmmu-pro-media-manifest";
import { z } from "../src/internal/zod";
import { readMediaMirrorEnv } from "./media-mirror";

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

export async function mirrorMmmuProMedia(options: {
  revision: string;
  out: string;
}) {
  const revision = z
    .string()
    .regex(/^[a-f0-9]{40}$/)
    .parse(options.revision);
  const env = readMediaMirrorEnv();
  const s3 = new S3Client(env);
  const images = [];
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
          if (bytes.byteLength === 0) {
            throw new Error(`MMMU Pro image ${row.id} is empty`);
          }
          const sha256 = createHash("sha256").update(bytes).digest("hex");
          const contentType = Bun.file(source.pathname).type;
          const key = `${env.keyPrefix}mmmu-pro/${revision}/${sha256}${extname(source.pathname)}`;
          await s3.file(key).write(bytes, { type: contentType });
          const url = `${env.publicBaseUrl}/${key}`;
          const check = await fetch(url);
          if (!check.ok) {
            await check.body?.cancel();
            throw new Error(
              `Published MMMU Pro image ${row.id}: HTTP ${check.status}`
            );
          }
          const publishedBytes = new Uint8Array(await check.arrayBuffer());
          if (
            check.headers.get("content-type")?.split(";")[0] !== contentType ||
            createHash("sha256").update(publishedBytes).digest("hex") !== sha256
          ) {
            throw new Error(
              `Published MMMU Pro image ${row.id} failed public readback verification`
            );
          }
          return {
            id: row.id,
            sourcePath: source.pathname,
            url,
            bytes: bytes.byteLength,
            contentType,
            sha256,
          };
        })
      );
      images.push(...batch);
    }
    process.stderr.write(
      `Mirrored ${images.length}/${total} MMMU Pro images\n`
    );
  }
  images.sort((a, b) => a.id.localeCompare(b.id));
  const manifest = {
    dataset: "MMMU/MMMU_Pro",
    config: "vision",
    split: "test",
    revision,
    manifestHash: createHash("sha256")
      .update(JSON.stringify(images))
      .digest("hex"),
    images,
  };
  buildMmmuProMediaManifest(manifest);
  await Bun.write(options.out, `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

if (import.meta.main) {
  const { values } = parseArgs({
    args: Bun.argv.slice(2),
    options: {
      revision: { type: "string" },
      out: {
        type: "string",
        default: "src/benchmarks/mmmu-pro-media-manifest.json",
      },
    },
  });
  await mirrorMmmuProMedia({
    revision: values.revision ?? "",
    out: values.out,
  });
}
