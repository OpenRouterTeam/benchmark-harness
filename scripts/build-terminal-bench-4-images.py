"""Build Terminal-Bench 4.0 task images into Modal's image store.

Usage:
  python3 scripts/build-terminal-bench-4-images.py --tasks-dir /path/to/terminal-bench/tasks \
      [--task NAME ...] [--out src/benchmarks/terminal-bench-4/image-ids.json] [--dry-run]

Requires MODAL_TOKEN_ID and MODAL_TOKEN_SECRET (or a Modal CLI profile). For each
runnable (non docker-compose) task this builds environment/Dockerfile and
tests/Dockerfile with their directories as build context, then records the
resulting Modal image IDs keyed by task id. The output JSON is what the harness
reads at run time, so re-run this whenever TERMINAL_BENCH_4_SOURCE_COMMIT changes.

MODAL_IMAGE_BUILDER_VERSION defaults to 2025.06. Older builder versions pip-install
Modal's client dependencies into the image, which would alter task environments.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

os.environ.setdefault("MODAL_IMAGE_BUILDER_VERSION", "2025.06")

import modal  # noqa: E402

APP_NAME = "terminal-bench-4-images"
COMPOSE_FILE = "environment/docker-compose.yaml"
DEFAULT_OUT = Path("src/benchmarks/terminal-bench-4/image-ids.json")
SOURCE_RE = re.compile(r'TERMINAL_BENCH_4_SOURCE_COMMIT\s*=\s*"([0-9a-f]{40})"')


def pinned_commit() -> str:
    text = Path("src/benchmarks/terminal-bench-4/tasks-source.ts").read_text()
    match = SOURCE_RE.search(text)
    if match is None:
        sys.exit("could not read TERMINAL_BENCH_4_SOURCE_COMMIT from tasks-source.ts")
    return match.group(1)


def checkout_commit(tasks_dir: Path) -> str:
    out = subprocess.run(
        ["git", "-C", str(tasks_dir), "rev-parse", "HEAD"],
        check=True,
        capture_output=True,
        text=True,
    )
    return out.stdout.strip()


def runnable_tasks(tasks_dir: Path) -> list[str]:
    return sorted(
        p.name
        for p in tasks_dir.iterdir()
        if p.is_dir()
        and not p.name.startswith(".")
        and (p / "task.toml").is_file()
        and not (p / COMPOSE_FILE).is_file()
    )


def load_existing(out: Path, commit: str) -> dict[str, dict[str, str]]:
    if not out.is_file():
        return {}
    data = json.loads(out.read_text())
    if data.get("sourceCommit") != commit:
        return {}
    return dict(data.get("images", {}))


def write_out(out: Path, commit: str, images: dict[str, dict[str, str]]) -> None:
    payload = {
        "sourceCommit": commit,
        "images": {k: images[k] for k in sorted(images)},
    }
    out.write_text(json.dumps(payload, indent=2) + "\n")


def build_task(app: modal.App, tasks_dir: Path, task: str) -> dict[str, str]:
    task_dir = tasks_dir / task
    agent = modal.Image.from_dockerfile(
        task_dir / "environment" / "Dockerfile",
        context_dir=task_dir / "environment",
    )
    verifier = modal.Image.from_dockerfile(
        task_dir / "tests" / "Dockerfile",
        context_dir=task_dir / "tests",
    )
    agent = agent.build(app)
    verifier = verifier.build(app)
    return {"agent": agent.object_id, "verifier": verifier.object_id}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tasks-dir", type=Path, required=True)
    parser.add_argument("--task", action="append", default=[])
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()

    tasks_dir: Path = args.tasks_dir.resolve()
    commit = pinned_commit()
    actual = checkout_commit(tasks_dir.parent)
    if actual != commit:
        sys.exit(f"tasks dir is at {actual}, expected pinned {commit}")

    runnable = runnable_tasks(tasks_dir)
    selected = args.task or runnable
    unknown = sorted(set(selected) - set(runnable))
    if unknown:
        sys.exit(f"unknown or docker-compose tasks: {', '.join(unknown)}")

    if args.dry_run:
        for task in selected:
            print(task)
        return 0

    images = load_existing(args.out, commit)
    failures: dict[str, str] = {}
    app = modal.App.lookup(APP_NAME, create_if_missing=True)

    def run(task: str) -> None:
        try:
            images[task] = build_task(app, tasks_dir, task)
            print(f"built {task}: {images[task]}", flush=True)
        except Exception as error:  # noqa: BLE001
            failures[task] = str(error)
            print(f"FAILED {task}: {error}", file=sys.stderr, flush=True)

    with modal.enable_output(), ThreadPoolExecutor(max_workers=args.concurrency) as pool:
        list(pool.map(run, selected))

    write_out(args.out, commit, images)
    if failures:
        print(f"{len(failures)} task(s) failed: {', '.join(sorted(failures))}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
