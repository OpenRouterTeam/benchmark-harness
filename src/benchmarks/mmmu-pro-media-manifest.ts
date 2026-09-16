import { z } from "../internal/zod";

const MmmuProMediaManifestSchema = z.object({
  dataset: z.literal("MMMU/MMMU_Pro"),
  config: z.literal("vision"),
  split: z.literal("test"),
  revision: z.string().regex(/^[a-f0-9]{40}$/),
  manifestHash: z.string().regex(/^[a-f0-9]{64}$/),
  images: z
    .array(
      z.object({
        id: z.string().min(1),
        sourcePath: z.string().startsWith("/cached-assets/MMMU/MMMU_Pro/"),
        url: z.url(),
      })
    )
    .min(1),
});

type ManifestImage = z.infer<
  typeof MmmuProMediaManifestSchema
>["images"][number];

export function buildMmmuProMediaManifest(raw: unknown) {
  const manifest = MmmuProMediaManifestSchema.parse(raw);
  const imageById = new Map<string, ManifestImage>();
  for (const image of manifest.images) {
    if (imageById.has(image.id)) {
      throw new TypeError(
        `MMMU Pro media manifest has duplicate id ${image.id}`
      );
    }
    if (
      !image.sourcePath.startsWith(
        `/cached-assets/MMMU/MMMU_Pro/--/${manifest.revision}/--/vision/test/`
      )
    ) {
      throw new TypeError(
        `MMMU Pro image ${image.id} does not match manifest revision`
      );
    }
    imageById.set(image.id, image);
  }
  return {
    revision: manifest.revision,
    manifestHash: manifest.manifestHash,
    imageById,
  };
}

export function mirroredMmmuProImage(
  manifest: ReturnType<typeof buildMmmuProMediaManifest>,
  id: string,
  sourceUrl: string
): string {
  const image = manifest.imageById.get(id);
  if (image === undefined) {
    throw new TypeError(`MMMU Pro image ${id} is missing from media manifest`);
  }
  const source = new URL(sourceUrl);
  if (
    source.origin !== "https://datasets-server.huggingface.co" ||
    source.pathname !== image.sourcePath
  ) {
    throw new TypeError(
      `MMMU Pro image ${id} changed; regenerate media manifest for the current dataset revision`
    );
  }
  return image.url;
}
