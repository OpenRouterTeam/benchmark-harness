# OpenRouter Benchmark Harness

OpenRouter's internal benchmarking harness, externalized for transparency. We port benchmarks here so we can run them scalably on our infrastructure and iterate quickly.

```sh
bun install
OPENROUTER_API_KEY=... bun run bench -- --benchmark gpqa_diamond --model openai/gpt-4o-mini --limit 5
```

## Dataset caching

Benchmark datasets (HuggingFace rows pages, GitHub task repo checkouts) are
cached on disk so repeat runs don't re-download them. The cache lives at
`~/.cache/openrouter-bench-harness` by default.

| Variable | Default | Meaning |
| --- | --- | --- |
| `BENCH_DATASET_CACHE_DIR` | `~/.cache/openrouter-bench-harness` | Cache root |
| `BENCH_DATASET_CACHE_DISABLE` | unset | Set to `1` to bypass the cache entirely |
| `BENCH_HF_CACHE_TTL_MS` | `86400000` (24h) | Freshness window for HuggingFace pages fetched without a pinned `revision`; revision-pinned pages never expire unless this is set explicitly, in which case it applies to all entries |

GitHub task repos are checked out under `<cache>/repos/<benchmark>-<commit>`
and reused while the pinned commit matches; HuggingFace `/rows` pages are
stored under `<cache>/hf/<dataset>/<config>/<split>/<revision>/`.

See [CONTRIBUTING.md](CONTRIBUTING.md) before proposing changes. Report security issues privately as described in [SECURITY.md](SECURITY.md).
