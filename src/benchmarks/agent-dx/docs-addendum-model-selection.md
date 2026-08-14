# Choosing an OpenRouter model (read this first)

Model IDs go stale: models are deprecated, lose endpoints, and change capabilities. **Never hardcode a model ID from memory** — a model you remember (e.g. `openai/gpt-4o-mini`, `google/gemini-2.0-flash-001`, `openai/gpt-3.5-turbo`) may no longer have live endpoints, which fails with `404 No endpoints found for <model>`.

Instead, always select a model from the live models endpoint:

```bash
curl -s https://openrouter.ai/api/v1/models
```

Each entry includes everything needed to pick a suitable model:

- `id` — the model ID to use in requests
- `context_length` — maximum context window in tokens
- `supported_parameters` — capabilities; check for `tools` (tool calling), `structured_outputs` / `response_format`, `reasoning`, etc.
- `architecture.input_modalities` — check for `"image"` before sending images
- `pricing.prompt` / `pricing.completion` — USD per token (strings)

Example: cheapest tool-capable model with at least 128k context —

```ts
const { data } = await (
  await fetch("https://openrouter.ai/api/v1/models")
).json();
const eligible = data.filter(
  (m) =>
    m.context_length >= 128_000 &&
    m.supported_parameters?.includes("tools") &&
    m.pricing?.prompt !== undefined &&
    // Router meta-models (e.g. openrouter/auto) report a sentinel price of -1.
    Number(m.pricing.prompt) >= 0
);
eligible.sort((a, b) => Number(a.pricing.prompt) - Number(b.pricing.prompt));
const model = eligible[0].id;
```

For vision/image tasks, filter with `m.architecture?.input_modalities?.includes('image')` rather than guessing a vision model ID.

If a request fails with `No endpoints found`, the model ID is stale — re-query `/api/v1/models` and choose a currently available model.
