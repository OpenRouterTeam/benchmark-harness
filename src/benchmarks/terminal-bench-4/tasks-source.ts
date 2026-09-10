import { join } from "node:path";

import { makeTasksSource } from "../harbor/tasks-source";

export const TERMINAL_BENCH_4_SOURCE_REPO =
  "https://github.com/harbor-framework/terminal-bench.git" as const;

export const TERMINAL_BENCH_4_SOURCE_COMMIT =
  "452bf305c6daa62fc59061d22133a7cbc7c1572e" as const;

export const TERMINAL_BENCH_4_TASKS_SUBDIR = "tasks" as const;

const source = makeTasksSource({
  label: "terminal-bench-4",
  repoUrl: TERMINAL_BENCH_4_SOURCE_REPO,
  commit: TERMINAL_BENCH_4_SOURCE_COMMIT,
  tasksSubdir: TERMINAL_BENCH_4_TASKS_SUBDIR,
  envVar: "BENCH_TERMINAL_BENCH_4_TASKS_DIR",
  tmpPrefix: "terminal-bench-4-tasks-",
});

export const {
  ensureTasksCheckedOut,
  ensureTasksCheckedOutEffect,
  seedTasksRoot,
  resetCheckoutCache,
} = source;

export function tasksDir(root: string): string {
  return join(root, TERMINAL_BENCH_4_TASKS_SUBDIR);
}
