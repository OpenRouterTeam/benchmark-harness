import { TERMINAL_BENCH_4_SOURCE_COMMIT } from "./tasks-source";

export const DEFAULT_TERMINAL_BENCH_4_IMAGE_REPO =
  "ghcr.io/openrouterteam/terminal-bench-4" as const;

export interface TerminalBench4ImageTags {
  readonly agent: string;
  readonly verifier: string;
}

export function imageTags(
  imageRepo: string,
  taskId: string
): TerminalBench4ImageTags {
  const tag = TERMINAL_BENCH_4_SOURCE_COMMIT.slice(0, 12);
  return {
    agent: `${imageRepo}/${taskId}:${tag}`,
    verifier: `${imageRepo}/${taskId}-verifier:${tag}`,
  };
}
