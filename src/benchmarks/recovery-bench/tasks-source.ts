import { makeTasksSource } from "../harbor/tasks-source";

export const RECOVERY_BENCH_SOURCE_REPO =
  "https://github.com/letta-ai/recovery-bench" as const;

export const RECOVERY_BENCH_SOURCE_COMMIT =
  "c5f83f2ba4f882a9b544c7bf0fa9be1bc3859c78" as const;

export const RECOVERY_BENCH_INITIAL_RUN =
  "initial-claude-haiku-4-5-20251001-20260303_194859" as const;

export const TERMINAL_BENCH_2_SOURCE_REPO =
  "https://github.com/laude-institute/terminal-bench-2.git" as const;

export const TERMINAL_BENCH_2_SOURCE_COMMIT =
  "69671fbaac6d67a7ef0dfec016cc38a64ef7a77c" as const;

const source = makeTasksSource({
  label: "recovery-bench",
  repoUrl: TERMINAL_BENCH_2_SOURCE_REPO,
  commit: TERMINAL_BENCH_2_SOURCE_COMMIT,
  tasksSubdir: ".",
  envVar: "BENCH_RECOVERY_BENCH_TASKS_DIR",
  tmpPrefix: "recovery-bench-tasks-",
});

export const {
  ensureTasksCheckedOut,
  ensureTasksCheckedOutEffect,
  seedTasksRoot,
  resetCheckoutCache,
} = source;
