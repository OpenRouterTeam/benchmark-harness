# Media mirror scripts

`mirror-vgi-bench-media.ts` and `mirror-mmmu-pro-media.ts` copy a benchmark's media from its upstream source into an OpenRouter-operated, publicly readable object store and write a committed manifest (`src/benchmarks/**/<benchmark>-media-manifest.json`). At run time the benchmark selects the manifest whose `revision` matches the requested dataset revision, sends models the mirror URLs instead of upstream URLs, and stamps `media_manifest_hash` and `dataset_revision` on every sample. Any other revision bypasses the mirror. The shared plumbing (environment, Hugging Face row paging with retries, download, SHA-256, upload-if-absent, public readback verification, manifest hashing) lives in `media-mirror.ts`.

## Storage

Each benchmark gets its own Cloudflare R2 bucket and public custom domain, so licensing, takedowns, and access tokens stay isolated per benchmark:

| Benchmark | Bucket | Public origin | Object keys |
| --- | --- | --- | --- |
| VGI-Bench | `vgi-bench-mirror` | `https://vgi-bench-mirror.openrouter.ai` | `<videoId>.mp4` |
| MMMU Pro (vision) | `mmmu-pro-mirror` | `https://mmmu-pro-mirror.openrouter.ai` | `mmmu-pro/<revision>/<sha256>.<ext>` |

To provision a new bucket in the Cloudflare dashboard: R2 > Create bucket; bucket Settings > Custom Domains > connect `<bucket>.openrouter.ai` (this is what makes objects publicly readable); Manage R2 API Tokens > Create API token with Object Read & Write scoped to that bucket only.

## Environment

Both scripts read the same variables, so set them per run for the target bucket:

| Variable | Value |
| --- | --- |
| `BENCH_MEDIA_S3_ENDPOINT` | `https://<cloudflare-account-id>.r2.cloudflarestorage.com` |
| `BENCH_MEDIA_S3_BUCKET` | Bucket name from the table above |
| `BENCH_MEDIA_S3_ACCESS_KEY_ID` / `BENCH_MEDIA_S3_SECRET_ACCESS_KEY` | R2 API token scoped to that bucket |
| `BENCH_MEDIA_PUBLIC_BASE_URL` | Public origin from the table above |
| `BENCH_MEDIA_KEY_PREFIX` | Optional. Leave unset; objects go at the bucket root or under the script's own prefix |
| `HF_TOKEN` | Optional. Only `mirror-vgi-bench-media.ts` reads it |

## Running

```sh
bun run mirror-vgi-bench-media -- --revision v1.0.1
bun run mirror-mmmu-pro-media                      # pinned to MMMU_PRO_DEFAULT_REVISION
bun run mirror-mmmu-pro-media -- --dry-run         # fetch, hash, verify revision; no writes
bun run mirror-mmmu-pro-media -- --force           # re-upload even when an object of the same size exists
```

Both scripts skip uploading an object whose stored size already matches (`--force` overrides), accept `--dry-run`, and print progress to stderr. They differ where the datasets differ:

- VGI-Bench takes any revision string, picks a downscaled or original source per video, and records videos it could not fetch under `unresolved` while still writing the manifest. The process exit code reflects the failures.
- MMMU Pro requires a 40-hex commit revision, checks the `x-revision` response header and every image's cached-asset path against it, re-reads every uploaded object from the public URL and compares content type and SHA-256, and fails without writing a manifest if any of the fixed 1,730 rows is missing. Keys are content addressed, so re-running against the same revision is a verification pass.

Commit the resulting manifest JSON with the code change that consumes it.
