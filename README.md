# OpenRouter Benchmark Harness

OpenRouter's internal benchmarking harness, externalized for transparency. We port benchmarks here so we can run them scalably on our infrastructure and iterate quickly.

```sh
bun install
OPENROUTER_API_KEY=... bun run bench -- --benchmark gpqa_diamond --model openai/gpt-4o-mini --limit 5
```

See [CONTRIBUTING.md](CONTRIBUTING.md) before proposing changes. Report security issues privately as described in [SECURITY.md](SECURITY.md).

## DeepSearchQA

DeepSearchQA uses the official 900-row [Google DeepSearchQA dataset](https://huggingface.co/datasets/google/deepsearchqa), `google/gemini-2.5-flash`, and the grading prompt from the [official starter notebook](https://www.kaggle.com/code/andrewmingwang/deepsearchqa-starter-code). The primary score is macro per-question `f1_score`; macro precision and recall are reported alongside the paper's four categorical rates. Generic `accuracy` remains the strict Fully Correct rate and is not the primary DSQA score.

The answer type is provided only to the judge, not to the model under test. Exact dataset and judge-prompt hashes are available from `@openrouter/bench-harness/benchmarks/search/provenance`. See the [paper](https://arxiv.org/abs/2601.20975) for the evaluation methodology.
