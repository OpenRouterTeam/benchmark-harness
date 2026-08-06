# Contributing

Contributions are welcome through GitHub pull requests.

## Setup

Use the Bun version declared in `package.json`.

```sh
bun install --frozen-lockfile
```

## Before opening a pull request

```sh
bun run format:check
bun run check
bun run typecheck
bun test
bun run build
```

Keep unit tests deterministic and credential-free. Do not commit API keys, private benchmark results, or dataset contents that cannot be redistributed.

New benchmarks must document their source, dataset and code licenses, evaluation method, and enough information to reproduce expected behavior. Changes to solvers, scorers, prompts, or datasets should include regression tests and explain any expected score changes.

By contributing, you agree that your contribution is licensed under the repository's Apache-2.0 license.
