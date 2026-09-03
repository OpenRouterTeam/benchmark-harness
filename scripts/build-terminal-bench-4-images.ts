import { existsSync } from "node:fs";
import { join } from "node:path";

import { listTaskIds } from "../src/benchmarks/terminal-bench-4/dataset";
import {
  DEFAULT_TERMINAL_BENCH_4_IMAGE_REPO,
  imageTags,
} from "../src/benchmarks/terminal-bench-4/images";
import {
  ensureTasksCheckedOut,
  tasksDir,
} from "../src/benchmarks/terminal-bench-4/tasks-source";

interface BuildOptions {
  readonly repo: string;
  readonly tasks: readonly string[];
  readonly dryRun: boolean;
  readonly concurrency: number;
}

interface ImageBuild {
  readonly tag: string;
  readonly contextDir: string;
  readonly dockerfile: string;
}

function requireValue(flag: string, value: string | undefined): string {
  if (value === undefined) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

function parseConcurrency(raw: string): number {
  const value = Number.parseInt(raw, 10);
  if (!Number.isInteger(value) || value < 1) {
    throw new Error(`invalid --concurrency: ${raw}`);
  }
  return value;
}

function parseArgs(argv: readonly string[]): BuildOptions {
  let repo: string = DEFAULT_TERMINAL_BENCH_4_IMAGE_REPO;
  const tasks: string[] = [];
  let dryRun = false;
  let concurrency = 2;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const next = argv[i + 1];
    switch (arg) {
      case "--repo": {
        repo = requireValue(arg, next);
        i++;
        break;
      }
      case "--task": {
        tasks.push(requireValue(arg, next));
        i++;
        break;
      }
      case "--concurrency": {
        concurrency = parseConcurrency(requireValue(arg, next));
        i++;
        break;
      }
      case "--dry-run": {
        dryRun = true;
        break;
      }
      default: {
        throw new Error(`unknown argument: ${arg}`);
      }
    }
  }
  return { repo, tasks, dryRun, concurrency };
}

export function taskImageBuilds(
  repo: string,
  tasksRoot: string,
  taskId: string
): readonly ImageBuild[] {
  const taskDir = join(tasksRoot, taskId);
  const tags = imageTags(repo, taskId);
  return [
    {
      tag: tags.agent,
      contextDir: join(taskDir, "environment"),
      dockerfile: join(taskDir, "environment", "Dockerfile"),
    },
    {
      tag: tags.verifier,
      contextDir: join(taskDir, "tests"),
      dockerfile: join(taskDir, "tests", "Dockerfile"),
    },
  ];
}

async function run(cmd: string, args: readonly string[]): Promise<void> {
  const proc = Bun.spawn([cmd, ...args], {
    stdout: "inherit",
    stderr: "inherit",
  });
  const code = await proc.exited;
  if (code !== 0) {
    throw new Error(`${cmd} ${args.join(" ")} exited with ${code}`);
  }
}

async function buildAndPush(build: ImageBuild, dryRun: boolean): Promise<void> {
  if (!existsSync(build.dockerfile)) {
    throw new Error(`missing Dockerfile: ${build.dockerfile}`);
  }
  const args = [
    "buildx",
    "build",
    "--platform",
    "linux/amd64",
    "--file",
    build.dockerfile,
    "--tag",
    build.tag,
    "--push",
    build.contextDir,
  ];
  if (dryRun) {
    console.log(`docker ${args.join(" ")}`);
    return;
  }

  await run("docker", args);
}

async function runBounded(
  jobs: readonly (() => Promise<void>)[],
  concurrency: number
): Promise<readonly PromiseSettledResult<void>[]> {
  const results: PromiseSettledResult<void>[] = new Array(jobs.length);
  let next = 0;
  const workers = Array.from(
    { length: Math.min(concurrency, jobs.length) },
    async () => {
      while (next < jobs.length) {
        const idx = next++;
        const job = jobs[idx];
        if (job === undefined) {
          return;
        }
        try {
          await job();
          results[idx] = { status: "fulfilled", value: undefined };
        } catch (reason) {
          results[idx] = { status: "rejected", reason };
        }
      }
    }
  );
  await Promise.all(workers);
  return results;
}

async function main(): Promise<void> {
  const opts = parseArgs(process.argv.slice(2));
  const root = ensureTasksCheckedOut();
  const tasksRoot = tasksDir(root);
  const taskIds = listTaskIds(tasksRoot, opts.tasks);
  const missing = opts.tasks.filter((id) => !taskIds.includes(id));
  if (missing.length > 0) {
    throw new Error(
      `unknown or compose-only tasks: ${missing.join(", ")} (compose tasks are unsupported)`
    );
  }
  const builds = taskIds.flatMap((id) =>
    taskImageBuilds(opts.repo, tasksRoot, id)
  );
  console.log(
    `${opts.dryRun ? "planning" : "building"} ${builds.length} images for ${taskIds.length} tasks -> ${opts.repo}`
  );
  const results = await runBounded(
    builds.map((b) => () => buildAndPush(b, opts.dryRun)),
    opts.concurrency
  );
  const failed = results.flatMap((r, i) =>
    r.status === "rejected" ? [{ build: builds[i], reason: r.reason }] : []
  );
  for (const f of failed) {
    console.error(`FAILED ${f.build?.tag}: ${String(f.reason)}`);
  }
  if (failed.length > 0) {
    process.exitCode = 1;
  }
}

if (import.meta.main) {
  await main();
}
