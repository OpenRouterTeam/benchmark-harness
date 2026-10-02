# toolcall_formats

Serving-stack check, not a capability benchmark: 27 tool-call scenarios × 5 encodings of optional fields (`omitted`, `anyof_null`, `anyof_null_strict`, `type_array_null`, `type_array_null_strict`) = 135 samples. The same model scores differently depending on whether the provider constrains tool-argument decoding to the JSON Schema it was sent, so pin `providerOnly` or `endpointId` and state the pin alongside any number.

## Source & license

The cases are synthetic, written by OpenRouter, and live in `cases.ts`. There is no external dataset. They are released under this repository's license. Sample ids (`toolcall_formats-<scenario>-<variant>`) are stable. Never rename one; add new scenarios instead.

## Evaluation method

- Temperature is fixed at 0. Each sample sends one system message, one user prompt, and the scenario's tools encoded with the sample's variant. `strict` is always sent explicitly, because the Responses API defaults an absent `strict` to `true`.
- The nullable encodings mark optional fields required, reproducing coding-agent strict-mode rewrites (for example pi's `anyOf [number, null]`), which is where broken grammars and parsers show up.
- `*_omit_requested` scenarios ask the model to leave optional arguments out. Under the nullable encodings those keys are still `required`, so only a stack that constrains decoding to the schema emits them. Under `omitted`, leaving them out is correct.
- `*_wrong_type_requested` scenarios ask for a scalar as a JSON string (`"50"`). A constrained stack still emits the schema type, so a string there is a `schema_violation`.
- `scorer.ts` validates each returned call against the exact schema sent (`json-schema.ts`), then against the expected values. Expected calls list only values the prompt states unambiguously. Keys the prompt leaves unset, or asks to omit, are left out of the expectation, so the schema still enforces their presence and type but any value the model picks passes.
- Failure classes: `no_tool_call`, `unknown_tool`, `invalid_json`, `schema_violation`, `call_count`, `wrong_value`. Format classes point at the serving stack; `wrong_value` usually points at the model.

## Config options

None beyond the shared model-benchmark options. Temperature is not configurable.
