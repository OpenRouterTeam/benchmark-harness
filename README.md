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
| `BENCH_HF_CACHE_TTL_MS` | `86400000` (24h) | Freshness window for HuggingFace pages fetched without a pinned `revision`; revision-pinned pages never expire |

GitHub task repos are cloned into a private staging directory and atomically
published under `<cache>/repos/<benchmark>-<commit>`, so concurrent runs
never clobber each other's checkouts; they are reused while the pinned commit
matches. HuggingFace `/rows` pages are stored under
`<cache>/hf/<token-scope>/<dataset>/<config>/<split>/<revision>/`, where
`<token-scope>` is `anon` for anonymous requests or a hash of the
`HF_TOKEN` used, so gated contents are never served across token scopes.
Cache files are written with owner-only permissions. Under `bun test` the
cache is disabled by default unless `BENCH_DATASET_CACHE_DIR` is set (or
`BENCH_DATASET_CACHE_DISABLE=0` opts in explicitly).

See [CONTRIBUTING.md](CONTRIBUTING.md) before proposing changes. Report security issues privately as described in [SECURITY.md](SECURITY.md).
