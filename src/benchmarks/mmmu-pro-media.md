# MMMU Pro vision media mirror

Hugging Face viewer image URLs expire after one hour; dataset pages can remain cached for 24 hours or indefinitely when revision-pinned. Follow VGI-Bench: publish the original media once and resolve sample IDs through the published manifest.

Source: [MMMU/MMMU_Pro](https://huggingface.co/datasets/MMMU/MMMU_Pro), `vision/test`; the dataset declares Apache-2.0. Images are copied without resizing or re-encoding. The generated manifest records the source revision and paths, stable URLs, byte counts, content types, and SHA-256 hashes. At runtime only URL mappings and provenance are retained, as in VGI.

## Publish

Use the existing VGI mirror's `BENCH_MEDIA_S3_ENDPOINT`, `BENCH_MEDIA_S3_BUCKET`, `BENCH_MEDIA_S3_ACCESS_KEY_ID`, `BENCH_MEDIA_S3_SECRET_ACCESS_KEY`, and `BENCH_MEDIA_PUBLIC_BASE_URL`. `BENCH_MEDIA_KEY_PREFIX` is optional. These are preparation-script settings, not worker or benchmark configuration.

```sh
bun run mirror-mmmu-pro-media \
  --revision=563f3e84bb3b90893083a1f039cfa13077f2302b
```

The command downloads each image, writes it under `<prefix>mmmu-pro/<revision>/<sha256>.<extension>`, and checks the bytes and content type through the public URL. It writes `src/benchmarks/mmmu-pro-media-manifest.json` only after every image passes. A failed attempt leaves any existing manifest unchanged; rerun the command to retry. Uploaded objects without a published manifest are unused. No staging directory or candidate public URLs are needed.

HF's `/rows` ignores `revision=`. The script checks the actual `x-revision` header and source paths; if the dataset has advanced, review the new revision before preparing its media. The runtime also requires each source path to match the manifest, ignoring expiring query parameters. Missing images/IDs and changed revisions fail explicitly.

## Activate after publication

Publication requires write access to the existing public mirror. The private, expiring GCS dataset-cache bucket is unsuitable. Until publication and representative provider fetches succeed, keep this PR in draft and leave the default dataset route unchanged.

After publication, import the generated JSON in `mmmu-pro-vision.ts`, validate it with `buildMmmuProMediaManifest`, and use it as the default `mediaManifest` inside `makeMmmuProVisionDatasetLayer`. The adapter already isolates its cache by revision and records `media_manifest_hash` and `dataset_revision`. No openrouter-web caller flag is needed. Bun/S3 stay in `scripts/`; the shared HF loader, cache format, providers, and deployment configuration are unaffected.
