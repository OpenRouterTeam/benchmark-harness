# MMMU Pro vision media preparation

MMMU Pro's Hugging Face viewer returns image URLs valid for one hour. Dataset pages can remain cached for 24 hours, or indefinitely when revision-pinned. The replacement follows VGI-Bench: publish original media once, then resolve sample IDs to stable URLs through a checksum-verified manifest. Image bytes do not belong in dataset row JSON, model-message history, or result Parquet.

Source: [MMMU/MMMU_Pro](https://huggingface.co/datasets/MMMU/MMMU_Pro), `vision`, `test`; the source dataset declares Apache-2.0. The preparation command copies original bytes without resizing or re-encoding. The manifest records source paths without signed query parameters, source revision, byte counts, content types, and SHA-256 hashes.

## Prepare without storage credentials

Choose an immutable source revision and an intended public base URL:

```sh
bun run mirror-mmmu-pro-media --prepare \
  --directory=/tmp/mmmu-pro-media \
  --revision=563f3e84bb3b90893083a1f039cfa13077f2302b \
  --public-base-url=https://vgi-bench-mirror.openrouter.ai
```

This writes original image files and a candidate `manifest.json` to the staging directory. It does not upload anything or change the runtime's media URLs. The public URL above is the existing VGI mirror; the staging step does not establish write access to its backing bucket. The publisher can select another public base URL and recalculates the manifest hash accordingly.

HF's `/rows` endpoint ignores `revision=`. Preparation therefore checks the actual `x-revision` header and each image's source path against the requested revision. If the dataset has advanced, select and review the new revision rather than assuming the viewer can read an old one.

## Publish and verify

Use the same storage configuration as the VGI mirror:

- `BENCH_MEDIA_S3_ENDPOINT`
- `BENCH_MEDIA_S3_BUCKET`
- `BENCH_MEDIA_S3_ACCESS_KEY_ID`
- `BENCH_MEDIA_S3_SECRET_ACCESS_KEY`
- `BENCH_MEDIA_PUBLIC_BASE_URL`
- Optional `BENCH_MEDIA_KEY_PREFIX`

```sh
bun run mirror-mmmu-pro-media --directory=/tmp/mmmu-pro-media
```

The publisher verifies each staged file's size and SHA-256, uploads it under `<prefix>mmmu-pro/<revision>/<content-hash>.<extension>`, and downloads the public URL to verify the same bytes. Only after every image succeeds does it write `src/benchmarks/mmmu-pro-media-manifest.json`. An existing object with the same size is reused, but public checksum verification still runs.

The private GCS dataset-cache bucket is disposable, blocks public access, and has a 30-day lifecycle. It is not the public media destination.

## Activation is gated on publication

Until the objects have passed public readback and representative provider-fetch checks, keep the replacement PR in draft and leave the default benchmark route unchanged. Do not commit a candidate staging manifest as a published manifest.

After publication, import the generated JSON in `mmmu-pro-vision.ts`, validate it with `buildMmmuProMediaManifest`, and use it as the default `mediaManifest` inside `makeMmmuProVisionDatasetLayer`. The adapter already accepts that resource, uses its revision for cache isolation, substitutes its URLs, and records `media_manifest_hash` and `dataset_revision` in result metadata. No public benchmark-config flag or caller change in openrouter-web is required.

The source image path must match the manifest entry even when its signature has expired. Changed revisions, missing IDs, and missing images fail explicitly; they never silently fall back to a signed source URL in a mirrored run.

The adapter's network-free tests verify unchanged prompts, answers, image detail, and metadata, as well as wrong-revision and missing-image failures. Before activation, exercise cached old URLs, a real provider fetch, and compact result messages using the published manifest. The shared HF loader and its pagination remain unchanged by this replacement; range overfetch is a separate pre-existing issue and is not needed to solve image expiry with stable references.
