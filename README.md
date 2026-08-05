# OpenRouter Benchmark Harness

OpenRouter's internal benchmarking harness, externalized for transparency. We port benchmarks here so we can run them scalably on our infrastructure and iterate quickly.

```sh
bun install
OPENROUTER_API_KEY=... bun run bench -- --benchmark gpqa_diamond --model openai/gpt-4o-mini --limit 5
```
