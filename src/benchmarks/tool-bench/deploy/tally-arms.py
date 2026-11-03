#!/usr/bin/env python3
"""Tally deploy/smoke-arms.sh output: per model x arm pass rate (Wilson 95%), overflow and fail
counts, prompt tokens and cost per run (universal: the router's usage.deferred_tools_details totals,
hidden search turns included), and paired per-task differences with a seeded bootstrap.

  deploy/tally-arms.py /tmp/tool-bench-arms/matrix/haiku /tmp/tool-bench-arms/matrix/kimi

An overflow is a run whose harness saw a context-length error and made no tool call. The world's
own context_overflow outcome needs an empty tools-container journal, which the MCP servers' startup
handshakes rule out, so it is derived here from the run's .meta.
"""
import collections
import glob
import json
import math
import os
import random
import sys

ARMS = ['universal', 'native', 'off']


def wilson(hits, n):
    if n == 0:
        return float('nan'), float('nan')
    z = 1.96
    p = hits / n
    denominator = 1 + z * z / n
    center = (p + z * z / (2 * n)) / denominator
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / denominator
    return center - half, center + half


def read_run(prefix):
    reward = json.load(open(prefix + '.reward.json'))
    meta = dict(
        part.split('=', 1)
        for part in open(prefix + '.meta').read().split()
        if '=' in part
    )
    prompt = cost = 0.0
    served = requests = 0
    for line in open(prefix + '.usage.jsonl'):
        if not line.strip():
            continue
        request = json.loads(line)
        requests += 1
        usage = request.get('usage') or {}
        details = usage.get('deferred_tools_details')
        native = sum(usage.get(key) or 0 for key in ('input_tokens', 'cache_read_input_tokens', 'cache_creation_input_tokens'))
        prompt += details['prompt_tokens'] if details else native
        served += 1 if details else 0
        cost += usage.get('cost') or 0
    passed = reward['reward'] == 1.0
    overflow = not passed and meta.get('overflow') == '1' and meta.get('tool_uses') == '0'
    return {
        'passed': passed,
        'overflow': overflow,
        'prompt': prompt,
        'cost': cost,
        'served': served,
        'requests': requests,
        'mcp_bypass': reward.get('mcp_bypass', 0),
    }


def split_name(name):
    # smoke-arms.sh writes <task>_<arm>, or <task>_<arm>_<run> when RUN is set.
    head, last = name.rsplit('_', 1)
    if last in ARMS:
        return head, last, '0'
    task, arm = head.rsplit('_', 1)
    return task, arm, last


def collect(directory):
    runs = []
    for path in sorted(glob.glob(os.path.join(directory, '*.reward.json'))):
        prefix = path[: -len('.reward.json')]
        name = os.path.basename(prefix)
        task, arm, run = split_name(name)
        runs.append({'model': os.path.basename(directory), 'task': task, 'arm': arm, 'run': run, **read_run(prefix)})
    return runs


def paired(runs, a, b, value):
    per_task = collections.defaultdict(dict)
    for run in runs:
        per_task[run['task']].setdefault(run['arm'], []).append(value(run))
    deltas = [
        sum(arms[a]) / len(arms[a]) - sum(arms[b]) / len(arms[b])
        for arms in per_task.values()
        if a in arms and b in arms
    ]
    if not deltas:
        return 'n/a'
    rng = random.Random(20261031)
    means = sorted(sum(rng.choice(deltas) for _ in deltas) / len(deltas) for _ in range(4000))
    return deltas, sum(deltas) / len(deltas), means[100], means[3899]


def main():
    for directory in sys.argv[1:]:
        runs = collect(directory)
        model = os.path.basename(directory)
        print(f'== {model}: {len(runs)} runs')
        print(f"{'arm':10} {'n':>3} {'passed [95%]':>16} {'overflow':>8} {'fail':>4} {'mean prompt':>12} {'mean $':>8} {'$/pass':>8} {'served':>9}")
        for arm in ARMS:
            group = [run for run in runs if run['arm'] == arm]
            if not group:
                continue
            n = len(group)
            passes = sum(run['passed'] for run in group)
            low, high = wilson(passes, n)
            overflows = sum(run['overflow'] for run in group)
            cost = sum(run['cost'] for run in group)
            print(
                f"{arm:10} {n:>3} {f'{100 * passes / n:.0f}% [{100 * low:.0f},{100 * high:.0f}]':>16} {overflows:>8} {n - passes - overflows:>4} "
                f"{sum(run['prompt'] for run in group) / n:>12,.0f} {cost / n:>8.4f} {(cost / passes if passes else float('nan')):>8.4f} "
                f"{sum(run['served'] for run in group)}/{sum(run['requests'] for run in group):<6}"
            )
        for a, b in (('universal', 'native'), ('universal', 'off')):
            for label, value, scale in (('passed', lambda run: float(run['passed']), 100), ('cost $', lambda run: run['cost'], 1)):
                result = paired(runs, a, b, value)
                if result == 'n/a':
                    continue
                deltas, mean, low, high = result
                unit = 'pp' if scale == 100 else ''
                print(f'  {a} - {b} {label:7} {mean * scale:+.3f}{unit} [{low * scale:+.3f}, {high * scale:+.3f}] over {len(deltas)} tasks')
        print()


if __name__ == '__main__':
    main()
