# OpenRouter Benchmark Harness

OpenRouter's internal benchmarking harness, externalized for transparency. We port benchmarks here so we can run them scalably on our infrastructure and iterate quickly.

```sh
bun install
OPENROUTER_API_KEY=... bun run bench -- --benchmark gpqa_diamond --model openai/gpt-4o-mini --limit 5
```

See [CONTRIBUTING.md](CONTRIBUTING.md) before proposing changes. Report security issues privately as described in [SECURITY.md](SECURITY.md).

## Jev cost tiers

Select a Jev tier with `--model typesafe/jev-router --cost-tier low`, `medium`, or `high`. The harness sends `plugins: [{ "id": "jev-router", "cost_tier": "high" }]` for high, including subsequent turns. Omitting the tier preserves the API's configured default. Cost tier and reasoning effort are separate settings.

To benchmark the current live policy, leave `experimentIds` unset. A Jev experiment selects its own routing configuration before the cost tier is resolved; combining an old experiment with `costTier: "high"` does not select the current live high configuration.

For SWE Atlas and Deep SWE, the built-in `mini_swe` agent forwards the tier through the harness model client. The `pi` sandbox agent forwards it through request plugins; agents that cannot forward plugins reject an explicit tier rather than silently using the API default.
