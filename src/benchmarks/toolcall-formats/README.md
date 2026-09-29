# toolcall_formats

Single-turn serving-stack check for tool calling. Each sample sends one prompt plus a small set of function tools and scores the returned `tool_calls` deterministically. It is meant to catch broken tool-call parsers, grammar / constrained-decoding bugs and degraded deployments, not to rank model ability: the prompts are trivial and fully specify every argument.

## Grid

Every scenario in `cases.ts` is crossed with five schema variants that change how optional parameters are encoded:

| variant | optional field schema | `required` | `strict` |
| --- | --- | --- | --- |
| `omitted` | plain type | omitted | off |
| `anyof_null` | `anyOf: [<type>, {type: "null"}]` | all fields | off |
| `anyof_null_strict` | `anyOf: [<type>, {type: "null"}]` | all fields | on |
| `type_array_null` | `type: [<type>, "null"]` | all fields | off |
| `type_array_null_strict` | `type: [<type>, "null"]` | all fields | on |

The nullable variants mirror how agent harnesses (e.g. pi strict mode) rewrite optional parameters. Scenarios cover strings, numbers, integers, decimals, negatives, booleans, enums, bounds, arrays, arrays of objects, nested objects, Unicode/JSON-heavy strings, no-argument tools and parallel calls.

## Scoring

A sample is correct only if the calls parse as JSON, validate against the exact schema that was sent, use known tools, have the expected count and match the expected argument values (numbers numerically, primitive arrays as multisets, unset optionals may be omitted or `null`). The explanation starts with the failure class: `no_tool_call`, `unknown_tool`, `invalid_json`, `schema_violation`, `call_count` or `wrong_value`. `schema_violation` is the serving-stack signal (e.g. `"offset": "40"` for `anyOf [number, null]`); `wrong_value` is mostly model behaviour.

## Provenance and license

The dataset is synthetic and defined in this directory; it is released under this repository's license. No external data is downloaded.
