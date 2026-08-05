/**
 * Minimal UCI engine driver for the chess benchmark (Stockfish).
 *
 * Determinism: Threads=1 and fixed-depth search make bestmove/eval
 * reproducible for a given binary version — the evaluation baseline is
 * independent of wall-clock, so scores are comparable across runs and
 * across concurrent games.
 *
 * Concurrency: each UciEngine owns one child process; games spawn their own
 * engine pair (opponent + evaluator) and quit them in a finally block, so
 * concurrent games never share engine state.
 */
import type { ChildProcessByStdio } from "node:child_process";
import { spawn } from "node:child_process";
import type { Interface } from "node:readline";
import { createInterface } from "node:readline";
import type { Readable, Writable } from "node:stream";

/** Engine score for a position, from the side-to-move's perspective. */
export interface UciScore {
  /** Centipawns (positive = side to move is better). Mate maps to ±MATE_CP. */
  readonly cp: number;
  /** Moves until mate, signed (positive = side to move mates). */
  readonly mateIn?: number;
}

export interface UciSearchResult {
  /** Best move in UCI long algebraic (e2e4). */
  readonly bestmove: string;
  readonly score: UciScore;
}

export const MATE_CP = 10_000;

export function stockfishPath(): string {
  return process.env["STOCKFISH_PATH"] ?? "stockfish";
}

const UCI_TIMEOUT_MS = 30_000;

const SPAWN_FAILED_LINE = "\u0000uci-spawn-failed";

export class UciEngine {
  readonly #proc: ChildProcessByStdio<Writable, Readable, null>;
  readonly #rl: Interface;
  #listeners: ((line: string) => void)[] = [];
  #spawnError: Error | undefined;
  /* Set on a #send timeout: the engine may still emit lines for the
   * timed-out search, which would be mis-delivered to the NEXT #send as
   * answers to a different position. A timed-out engine is dead. */
  #dead: Error | undefined;

  private constructor(proc: ChildProcessByStdio<Writable, Readable, null>) {
    this.#proc = proc;
    /* stdin errors (EPIPE on a dead engine, ERR_STREAM_DESTROYED after a
     * failed spawn) must not become uncaught exceptions in the worker —
     * the spawn/exit paths already surface the actionable error. */
    proc.stdin.on("error", () => undefined);
    this.#rl = createInterface({ input: proc.stdout });
    this.#rl.on("line", (line) => {
      for (const listener of this.#listeners) {
        listener(line);
      }
    });
  }

  /**
   * Spawn + UCI handshake. Fails closed with a remediation message when the
   * binary is missing — a chess run must never silently score without its
   * engine.
   */
  static async start(): Promise<UciEngine> {
    const bin = stockfishPath();
    const proc = spawn(bin, [], { stdio: ["pipe", "pipe", "ignore"] });
    const engine = new UciEngine(proc);
    /* A spawn failure (missing binary) rejects every in-flight #send by
     * flushing its listeners with a poisoned line — no Promise.race needed,
     * and no listener leak on the losing branch. */
    proc.on("error", (error) => {
      engine.#spawnError = new Error(
        `stockfish not executable at "${bin}" (${error.message}). ` +
          "Install with `brew install stockfish` / `apt-get install stockfish`, or set STOCKFISH_PATH."
      );
      for (const listener of engine.#listeners) {
        listener(SPAWN_FAILED_LINE);
      }
    });
    try {
      await engine.#send("uci", (line) => line === "uciok");
      engine.#write("setoption name Threads value 1");
      await engine.#send("isready", (line) => line === "readyok");
    } catch (error) {
      engine.quit();
      throw error;
    }
    return engine;
  }

  #write(cmd: string): void {
    if (this.#proc.stdin.writable) {
      this.#proc.stdin.write(`${cmd}\n`);
    }
  }

  /** Send a command; resolve with all lines up to the one matching `done`. */
  #send(cmd: string, done: (line: string) => boolean): Promise<string[]> {
    return new Promise((resolve, reject) => {
      if (this.#dead !== undefined) {
        reject(this.#dead);
        return;
      }
      const lines: string[] = [];
      const listener = (line: string): void => {
        if (line === SPAWN_FAILED_LINE) {
          this.#listeners = this.#listeners.filter((l) => l !== listener);
          clearTimeout(timer);
          reject(this.#spawnError ?? new Error("uci engine failed to spawn"));
          return;
        }
        lines.push(line);
        if (done(line)) {
          this.#listeners = this.#listeners.filter((l) => l !== listener);
          clearTimeout(timer);
          resolve(lines);
        }
      };
      const timer = setTimeout(() => {
        this.#listeners = this.#listeners.filter((l) => l !== listener);
        /* Poison the engine: late lines from this search would otherwise be
         * delivered to the next #send as answers to a different position. */
        this.#dead = new Error(`UCI timeout on "${cmd}" — engine marked dead`);
        this.#proc.kill("SIGKILL");
        reject(this.#dead);
      }, UCI_TIMEOUT_MS);
      this.#listeners.push(listener);
      this.#write(cmd);
    });
  }

  /** Search a position to fixed depth: best move + score (side-to-move view). */
  async search(fen: string, depth: number): Promise<UciSearchResult> {
    this.#write(`position fen ${fen}`);
    const lines = await this.#send(`go depth ${depth}`, (line) =>
      line.startsWith("bestmove")
    );
    const last = lines.at(-1);
    const bestmove = last === undefined ? "" : (last.split(/\s+/)[1] ?? "");
    // The last `info` line carrying a score wins (deepest completed iteration).
    let score: UciScore = { cp: 0 };
    for (const line of lines) {
      /* Skip aspiration-window bound lines: a lowerbound/upperbound score is
       * a search bound, not the position's evaluation, and taking one as
       * the final score skews cpLoss/adjudication. */
      if (/\b(lowerbound|upperbound)\b/.test(line)) {
        continue;
      }
      const match = line.match(/\bscore (cp|mate) (-?\d+)/);
      if (match?.[1] === undefined || match[2] === undefined) {
        continue;
      }
      score =
        match[1] === "cp"
          ? { cp: Number(match[2]) }
          : {
              cp: Math.sign(Number(match[2])) * MATE_CP,
              mateIn: Number(match[2]),
            };
    }
    return { bestmove, score };
  }

  newGame(): void {
    this.#write("ucinewgame");
  }

  quit(): void {
    this.#write("quit");
    // Belt and braces: never leave engine processes behind if quit is ignored.
    const proc = this.#proc;
    setTimeout(() => proc.kill("SIGKILL"), 2000).unref();
    this.#rl.close();
  }
}
