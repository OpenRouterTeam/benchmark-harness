# Agent Guidelines

## Workflow

- Read the relevant implementation and tests before editing.
- Prefer the smallest backward-compatible change that solves the problem.
- Keep tests deterministic and network-free unless they are explicitly manual integration tests.
- Never commit API keys, private benchmark results, or restricted dataset contents.

## Code

- Preserve the repository's strict TypeScript and Effect patterns.
- Validate external model, provider, dataset, and filesystem data at runtime.
- Keep tests colocated as `*.test.ts` and add regression coverage for behavior changes.
- Treat solver, scorer, prompt, and dataset changes as benchmark behavior changes.
- Document dataset provenance and licensing for new benchmarks.

## Validation

Run `bun run format:check`, `bun run check`, `bun run typecheck`, `bun test`, and `bun run build` before submitting changes.
