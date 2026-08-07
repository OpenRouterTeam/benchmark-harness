# Agent Guidelines

## Workflow

- Read the relevant implementation and tests before editing.
- Prefer the smallest backward-compatible change that solves the problem.
- Keep tests deterministic and network-free unless they are explicitly manual integration tests.
- Never commit API keys, private benchmark results, or restricted dataset contents.

## Code

- Preserve the repository's strict TypeScript and Effect patterns.
- Do not re-enable the following ultracite lint rules in `oxlint.config.ts`; they are disabled because they misfire on Effect (they are not type-aware and match `.then`/`.catch`/callbacks/constructors by syntax alone, flagging Effect's `Schedule`/`Effect.async`/`Effect.runPromise`/`TaggedError` combinators and pushing code toward worse error handling instead of `catchAll`/`catchTag`):
  - `promise/prefer-await-to-then`
  - `promise/prefer-await-to-callbacks`
  - `promise/no-nesting`
  - `unicorn/throw-new-error` (fires on `TaggedError("Name")<{...}>()` factory calls)
- Validate external model, provider, dataset, and filesystem data at runtime.
- Keep tests colocated as `*.test.ts` and add regression coverage for behavior changes.
- Treat solver, scorer, prompt, and dataset changes as benchmark behavior changes.
- Document dataset provenance and licensing for new benchmarks.

## Validation

Run `bun run format:check`, `bun run check`, `bun run typecheck`, `bun test`, and `bun run build` before submitting changes.
