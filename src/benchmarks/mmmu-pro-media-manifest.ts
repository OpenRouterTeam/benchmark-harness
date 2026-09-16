import { z } from "../internal/zod";

export const MMMU_PRO_DATASET_PATH = "MMMU/MMMU_Pro";
export const MMMU_PRO_VISION_SUBSET = "vision";
export const MMMU_PRO_SPLIT = "test";
export const MMMU_PRO_DEFAULT_REVISION =
  "563f3e84bb3b90893083a1f039cfa13077f2302b";

const HF_ORIGIN = "https://datasets-server.huggingface.co";
const CACHED_ASSET_ROOT = `/cached-assets/${MMMU_PRO_DATASET_PATH}/`;

export function mmmuProCachedAssetPrefix(revision: string): string {
  return `${CACHED_ASSET_ROOT}--/${revision}/--/${MMMU_PRO_VISION_SUBSET}/${MMMU_PRO_SPLIT}/`;
}

const MmmuProMediaManifestSchema = z.object({
  dataset: z.literal(MMMU_PRO_DATASET_PATH),
  config: z.literal(MMMU_PRO_VISION_SUBSET),
  split: z.literal(MMMU_PRO_SPLIT),
  revision: z.string().regex(/^[a-f0-9]{40}$/),
  manifestHash: z.string().regex(/^[a-f0-9]{64}$/),
  images: z
    .array(
      z.object({
        id: z.string().min(1),
        sourcePath: z.string().startsWith(CACHED_ASSET_ROOT),
        url: z.url(),
      })
    )
    .min(1),
});

type ManifestImage = z.infer<
  typeof MmmuProMediaManifestSchema
>["images"][number];

export interface MmmuProMediaManifest {
  readonly revision: string;
  readonly manifestHash: string;
  readonly imageById: ReadonlyMap<string, ManifestImage>;
}

export function buildMmmuProMediaManifest(raw: unknown): MmmuProMediaManifest {
  const manifest = MmmuProMediaManifestSchema.parse(raw);
  const sourcePrefix = mmmuProCachedAssetPrefix(manifest.revision);
  const imageById = new Map<string, ManifestImage>();
  for (const image of manifest.images) {
    if (imageById.has(image.id)) {
      throw new TypeError(
        `MMMU Pro media manifest has duplicate id ${image.id}`
      );
    }
    if (!image.sourcePath.startsWith(sourcePrefix)) {
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
  manifest: MmmuProMediaManifest,
  id: string,
  sourceUrl: string
): string {
  const image = manifest.imageById.get(id);
  if (image === undefined) {
    throw new TypeError(`MMMU Pro image ${id} is missing from media manifest`);
  }
  const source = new URL(sourceUrl);
  if (source.origin !== HF_ORIGIN || source.pathname !== image.sourcePath) {
    throw new TypeError(
      `MMMU Pro image ${id} changed; regenerate media manifest for the current dataset revision`
    );
  }
  return image.url;
}

const MMMU_PRO_MEDIA_MANIFESTS: readonly MmmuProMediaManifest[] = [];

export function mmmuProMediaManifestFor(
  revision: string
): MmmuProMediaManifest | undefined {
  return MMMU_PRO_MEDIA_MANIFESTS.find(
    (manifest) => manifest.revision === revision
  );
}
