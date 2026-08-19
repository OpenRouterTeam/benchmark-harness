import { execFile, execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";

import { option, string } from "effect/Config";
import { TaggedError } from "effect/Data";
import type { Effect } from "effect/Effect";
import { async, fail, runSync, succeed, tryPromise } from "effect/Effect";
import { getOrNull } from "effect/Option";

import {
  datasetCacheRoot,
  publishStagedCheckout,
} from "../../datasets/local-cache";
import { runHarnessPromise } from "../../internal/effect-logger";
import { wLog } from "../../internal/log";

export interface TasksSourceConfig {
  readonly label: string;
  readonly repoUrl: string;
  readonly commit: string;
  readonly tasksSubdir: string;
  readonly envVar: string;
  readonly tmpPrefix: string;
}

export interface TasksSource {
  readonly ensureTasksCheckedOut: () => Promise<string>;
  readonly ensureTasksCheckedOutEffect: () => Effect<
    string,
    HarborTasksCheckoutError
  >;
  readonly seedTasksRoot: (root: string) => void;
  readonly resetCheckoutCache: () => void;
}

export class HarborTasksCheckoutError extends TaggedError(
  "HarborTasksCheckoutError"
)<{
  readonly message: string;
  readonly cause: unknown;
}> {}

export function makeTasksSource(config: TasksSourceConfig): TasksSource {
  let cacheRoot: string | undefined;
  let checkoutPromise: Promise<string> | undefined;
  const hasTasksDir = (dir: string): boolean => {
    try {
      return statSync(join(dir, config.tasksSubdir)).isDirectory();
    } catch {
      return false;
    }
  };
  const isAtPinnedCommit = (dir: string): boolean => {
    try {
      const result = execFileSync("git", ["-C", dir, "rev-parse", "HEAD"], {
        encoding: "utf8",
        timeout: 10000,
      });
      return result.trim() === config.commit;
    } catch {
      return false;
    }
  };
  const sharedCheckoutRoot = (): string | undefined => {
    const root = datasetCacheRoot();
    if (root === undefined) {
      return undefined;
    }
    return join(root, "repos", `${config.label}-${config.commit.slice(0, 12)}`);
  };
  const cloneInto = async (root: string): Promise<void> => {
    await runGit([
      "clone",
      "--depth",
      "1",
      "--filter=blob:none",
      config.repoUrl,
      root,
    ]);
    await runGit([
      "-C",
      root,
      "fetch",
      "--depth",
      "1",
      "origin",
      config.commit,
    ]);
    await runGit(["-C", root, "checkout", config.commit]);
  };
  const cloneTasks = async (): Promise<string> => {
    const override = getOrNull(runSync(string(config.envVar).pipe(option)));
    if (
      override !== null &&
      override.length > 0 &&
      isEmptyOrMissing(override)
    ) {
      await cloneInto(override);
      cacheRoot = override;
      return override;
    }
    const shared = sharedCheckoutRoot();
    if (shared === undefined) {
      const tmp = mkdtempSync(join(tmpdir(), config.tmpPrefix));
      await cloneInto(tmp);
      cacheRoot = tmp;
      return tmp;
    }
    mkdirSync(dirname(shared), { recursive: true });
    const staging = mkdtempSync(
      join(dirname(shared), `.${basename(shared)}.staging-`)
    );
    await cloneInto(staging);
    const root = publishStagedCheckout(
      staging,
      shared,
      () => isAtPinnedCommit(shared) && hasTasksDir(shared)
    );
    cacheRoot = root;
    return root;
  };
  const ensureTasksCheckedOut = (): Promise<string> => {
    if (cacheRoot && hasTasksDir(cacheRoot)) {
      return Promise.resolve(cacheRoot);
    }
    const override = getOrNull(runSync(string(config.envVar).pipe(option)));
    if (override && hasTasksDir(override) && isAtPinnedCommit(override)) {
      cacheRoot = override;
      return Promise.resolve(override);
    }
    const shared = sharedCheckoutRoot();
    if (
      shared !== undefined &&
      hasTasksDir(shared) &&
      isAtPinnedCommit(shared)
    ) {
      cacheRoot = shared;
      return Promise.resolve(shared);
    }
    if (
      override !== null &&
      override.length > 0 &&
      !isEmptyOrMissing(override)
    ) {
      wLog(
        "harbor tasks dir override not at pinned commit; cloning to a temp dir instead",
        {
          benchmark: config.label,
          env_var: config.envVar,
          bench_tasks_dir: override,
          pinned_commit: config.commit,
        }
      );
    }
    if (!checkoutPromise) {
      checkoutPromise = cloneTasks().catch((error: unknown) => {
        checkoutPromise = undefined;
        throw error;
      });
    }
    return checkoutPromise;
  };
  return {
    ensureTasksCheckedOut,
    ensureTasksCheckedOutEffect: () =>
      tryPromise({
        try: () => ensureTasksCheckedOut(),
        catch: (e: unknown) =>
          new HarborTasksCheckoutError({
            message: `Failed to check out ${config.label} tasks: ${String(e)}`,
            cause: e,
          }),
      }),
    seedTasksRoot: (root: string): void => {
      if (!hasTasksDir(root)) {
        return;
      }
      cacheRoot = root;
      checkoutPromise = Promise.resolve(root);
    },
    resetCheckoutCache: (): void => {
      cacheRoot = undefined;
      checkoutPromise = undefined;
    },
  };
}

function isEmptyOrMissing(path: string): boolean {
  try {
    return readdirSync(path).length === 0;
  } catch {
    return true;
  }
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
