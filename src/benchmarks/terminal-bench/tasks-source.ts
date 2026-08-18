import { execFile, execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { option, string } from "effect/Config";
import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { async, fail, runSync, succeed, tryPromise } from "effect/Effect";
import { getOrNull } from "effect/Option";

import {
  datasetCacheRoot,
  removeDirRecursive,
} from "../../datasets/local-cache";
import { runHarnessPromise } from "../../internal/effect-logger";

export const TERMINAL_BENCH_SOURCE_REPO =
  "https://github.com/harbor-framework/terminal-bench-2-1.git" as const;

export const TERMINAL_BENCH_SOURCE_COMMIT =
  "c5ee500c185224c97cd6caff7866a990a0057f41" as const;

export const TERMINAL_BENCH_TASKS_SUBDIR = "tasks" as const;

function sharedCheckoutRoot(): string | undefined {
  const root = datasetCacheRoot();
  if (root === undefined) {
    return undefined;
  }
  return join(
    root,
    "repos",
    `terminal-bench-${TERMINAL_BENCH_SOURCE_COMMIT.slice(0, 12)}`
  );
}

function resolveCacheRoot(): string {
  const override = getOrNull(runSync(string("BENCH_TASKS_DIR").pipe(option)));
  if (override && override.length > 0 && isEmptyOrMissing(override)) {
    return override;
  }
  const shared = sharedCheckoutRoot();
  if (shared !== undefined) {
    return shared;
  }
  return mkdtempSync(join(tmpdir(), "terminal-bench-2-1-tasks-"));
}

function isEmptyOrMissing(path: string): boolean {
  try {
    return readdirSync(path).length === 0;
  } catch {
    return true;
  }
}

let cacheRoot: string | undefined;

let checkoutPromise: Promise<string> | undefined;

export function ensureTasksCheckedOut(): Promise<string> {
  if (cacheRoot && hasTasks(cacheRoot)) {
    return Promise.resolve(cacheRoot);
  }
  const override = getOrNull(runSync(string("BENCH_TASKS_DIR").pipe(option)));
  if (override) {
    const resolved = resolveTasksDir(override);
    if (resolved !== undefined && isAtPinnedCommit(resolved)) {
      cacheRoot = resolved;
      return Promise.resolve(resolved);
    }
  }
  const shared = sharedCheckoutRoot();
  if (shared !== undefined && isAtPinnedCommit(shared)) {
    const resolved = resolveTasksDir(shared);
    if (resolved !== undefined) {
      cacheRoot = resolved;
      return Promise.resolve(resolved);
    }
  }
  if (!checkoutPromise) {
    checkoutPromise = cloneTasks().catch((error: unknown) => {
      checkoutPromise = undefined;
      throw error;
    });
  }
  return checkoutPromise;
}

function hasTasks(dir: string): boolean {
  try {
    return readdirSync(dir).some((entry) => {
      const p = join(dir, entry);
      return (
        statSync(p).isDirectory() && !entry.startsWith(".") && hasTaskToml(p)
      );
    });
  } catch {
    return false;
  }
}

function hasTaskToml(dir: string): boolean {
  try {
    return statSync(join(dir, "task.toml")).isFile();
  } catch {
    return false;
  }
}

function resolveTasksDir(base: string): string | undefined {
  if (hasTasks(base)) {
    return base;
  }
  const nested = join(base, TERMINAL_BENCH_TASKS_SUBDIR);
  if (hasTasks(nested)) {
    return nested;
  }
  return undefined;
}

function isAtPinnedCommit(dir: string): boolean {
  try {
    const result = execFileSync("git", ["-C", dir, "rev-parse", "HEAD"], {
      encoding: "utf8",
      timeout: 10000,
    });
    return result.trim() === TERMINAL_BENCH_SOURCE_COMMIT;
  } catch {
    return false;
  }
}

export function seedTasksDir(tasksDir: string): void {
  if (!hasTasks(tasksDir)) {
    return;
  }
  cacheRoot = tasksDir;
  checkoutPromise = Promise.resolve(tasksDir);
}

export function resetCheckoutCache(): void {
  cacheRoot = undefined;
  checkoutPromise = undefined;
}

async function cloneTasks(): Promise<string> {
  const root = resolveCacheRoot();
  if (!isEmptyOrMissing(root)) {
    const resolved = resolveTasksDir(root);
    if (resolved !== undefined && isAtPinnedCommit(resolved)) {
      cacheRoot = resolved;
      return resolved;
    }
    removeDirRecursive(root);
  }
  await runGit([
    "clone",
    "--depth",
    "1",
    "--filter=blob:none",
    TERMINAL_BENCH_SOURCE_REPO,
    root,
  ]);
  await runGit([
    "-C",
    root,
    "fetch",
    "--depth",
    "1",
    "origin",
    TERMINAL_BENCH_SOURCE_COMMIT,
  ]);
  await runGit(["-C", root, "checkout", TERMINAL_BENCH_SOURCE_COMMIT]);
  const tasksDir = join(root, TERMINAL_BENCH_TASKS_SUBDIR);
  cacheRoot = tasksDir;
  return tasksDir;
}

class GitError extends TaggedError("GitError")<{
  readonly message: string;
}> {}

function runGit(args: string[]): Promise<void> {
  return runHarnessPromise(
    async<void, GitError>((resolve) => {
      const proc = execFile("git", args, { maxBuffer: 64 * 1024 * 1024 });
      let stderr = "";
      proc.stderr?.on("data", (d: Buffer) => (stderr += d.toString()));
      proc.on("error", (err) =>
        resolve(
          fail(
            new GitError({
              message: `git ${args.join(" ")} failed: ${String(err)}`,
            })
          )
        )
      );
      proc.on("close", (code) => {
        if (code === 0) {
          resolve(succeed(undefined));
        } else {
          resolve(
            fail(
              new GitError({
                message: `git ${args[0]} exited ${code}: ${stderr.slice(-1000)}`,
              })
            )
          );
        }
      });
    })
  );
}

class TasksCheckoutError extends TaggedError("TasksCheckoutError")<{
  readonly message: string;
  readonly cause: unknown;
}> {}

export function ensureTasksCheckedOutEffect(): Effect<
  string,
  TasksCheckoutError
> {
  return tryPromise({
    try: () => ensureTasksCheckedOut(),
    catch: (e: unknown) =>
      new TasksCheckoutError({
        message: `Failed to check out terminal-bench tasks: ${String(e)}`,
        cause: e,
      }),
  });
}
