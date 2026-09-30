import type { Expr, Statement, Value } from "./language";
import { LanguageError, parse } from "./language";

export interface Provider {
  readonly write: (prompt: string, value: Value | undefined) => Promise<string>;
  readonly judge: (
    value: Value,
    labels: readonly string[]
  ) => Promise<Readonly<Record<string, number>>>;
}

export type EffectKind = "write" | "judge";

export interface RecordedEffect {
  readonly kind: EffectKind;
  readonly args: unknown;
  readonly result: unknown;
  readonly draw?: number;
}

export interface JudgeTraceDetail {
  readonly value: Value;
  readonly labels: readonly string[];
  readonly probabilities: Readonly<Record<string, number>>;
  readonly chosen: string | null;
  readonly threshold: number;
}

export type TraceEvent =
  | {
      readonly kind: "judge";
      readonly line: number;
      readonly detail: JudgeTraceDetail;
    }
  | { readonly kind: "write"; readonly line: number; readonly text: string }
  | { readonly kind: "print"; readonly line: number; readonly text: string }
  | {
      readonly kind: "assign";
      readonly line: number;
      readonly name: string;
      readonly value: Value;
    }
  | { readonly kind: "repeat"; readonly line: number; readonly text: string };

export interface Run {
  readonly version: 1;
  readonly source: string;
  readonly input: string;
  readonly tape: readonly RecordedEffect[];
  readonly output: readonly string[];
  readonly trace: readonly TraceEvent[];
}

export interface RunOptions {
  readonly input?: string;
  readonly replay?: readonly RecordedEffect[];
  readonly random?: () => number;
  readonly maxInputChars?: number;
  readonly maxEffects?: number;
}

export class RuntimeError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "RuntimeError";
  }
}

const DEFAULT_MAX_INPUT_CHARS = 6000;
const DEFAULT_MAX_EFFECTS = 12;
const MAX_STATEMENTS = 200;
const MAX_WRITE_CHARS = 12_000;
const MAX_WHILE_ITERATIONS = 5;
const DISTRIBUTION_TOLERANCE = 0.02;

export function distribution(
  raw: unknown,
  labels: readonly string[]
): Record<string, number> {
  if (raw === null || typeof raw !== "object") {
    throw new RuntimeError("Judge returned no probabilities.");
  }
  const obj: Record<string, unknown> = { ...raw };
  const values = labels.map((label) => {
    const n = obj[label];
    if (typeof n !== "number" || !Number.isFinite(n) || n < 0 || n > 1) {
      throw new RuntimeError("Judge returned invalid probabilities.");
    }
    return n;
  });
  const sum = values.reduce((acc, n) => acc + n, 0);
  if (Math.abs(sum - 1) > DISTRIBUTION_TOLERANCE) {
    throw new RuntimeError("Judge probabilities do not sum to one.");
  }
  return Object.fromEntries(
    labels.map((label, i) => [label, (values[i] ?? 0) / sum])
  );
}

function ifBranch(
  s: Extract<Statement, { kind: "if" }>,
  selected: string | null
): readonly Statement[] {
  if (selected === null) {
    return s.maybe;
  }
  return selected === s.question ? s.yes : s.no;
}

function argmax(
  labels: readonly string[],
  p: (label: string) => number
): string {
  let best = labels[0] ?? "";
  for (const label of labels) {
    if (p(label) > p(best)) {
      best = label;
    }
  }
  return best;
}

function sampleLabel(
  labels: readonly string[],
  p: (label: string) => number,
  draw: number
): string {
  let remaining = draw;
  for (const label of labels) {
    remaining -= p(label);
    if (remaining < 0) {
      return label;
    }
  }
  return labels.at(-1) ?? "";
}

class Interpreter {
  private readonly scopes: Map<string, Value>[] = [new Map()];
  private readonly tape: RecordedEffect[] = [];
  private readonly output: string[] = [];
  private readonly trace: TraceEvent[] = [];
  private calls = 0;
  private cursor = 0;
  private steps = 0;

  private readonly provider: Provider;
  private readonly input: string;
  private readonly options: RunOptions;

  constructor(provider: Provider, input: string, options: RunOptions) {
    this.provider = provider;
    this.input = input;
    this.options = options;
  }

  async run(source: string): Promise<Run> {
    const ast = parse(source);
    await this.execute(ast, false, false);
    if (this.options.replay && this.cursor !== this.options.replay.length) {
      throw new RuntimeError("Replay has unused model results.");
    }
    return {
      version: 1,
      source,
      input: this.input,
      tape: this.tape,
      output: this.output,
      trace: this.trace,
    };
  }

  private scopeOf(name: string): Map<string, Value> | undefined {
    return this.scopes.find((s) => s.has(name));
  }

  private async effect(
    kind: EffectKind,
    args: unknown,
    call: () => Promise<unknown>,
    chaos: boolean
  ): Promise<RecordedEffect> {
    this.calls++;
    const maxEffects = this.options.maxEffects ?? DEFAULT_MAX_EFFECTS;
    if (this.calls > maxEffects) {
      throw new RuntimeError(
        `Run stopped at the ${maxEffects} model-call limit.`
      );
    }
    const recorded = await this.recordOrReplay(kind, args, call, chaos);
    if (
      chaos &&
      (typeof recorded.draw !== "number" ||
        recorded.draw < 0 ||
        recorded.draw >= 1)
    ) {
      throw new RuntimeError("Invalid chaos draw in replay.");
    }
    this.tape.push(recorded);
    return recorded;
  }

  private async recordOrReplay(
    kind: EffectKind,
    args: unknown,
    call: () => Promise<unknown>,
    chaos: boolean
  ): Promise<RecordedEffect> {
    if (this.options.replay) {
      const saved = this.options.replay[this.cursor];
      this.cursor++;
      if (
        saved === undefined ||
        saved.kind !== kind ||
        JSON.stringify(saved.args) !== JSON.stringify(args)
      ) {
        throw new RuntimeError("Replay does not match this program and input.");
      }
      return structuredClone(saved);
    }
    const result = await call();
    if (!chaos) {
      return { kind, args, result };
    }
    return { kind, args, result, draw: (this.options.random ?? Math.random)() };
  }

  private async evaluate(e: Expr, line: number): Promise<Value> {
    switch (e.kind) {
      case "literal": {
        return e.value;
      }
      case "input": {
        return this.input;
      }
      case "variable": {
        const scope = this.scopeOf(e.name);
        const value = scope?.get(e.name);
        if (value === undefined) {
          throw new LanguageError(`Unknown variable ${e.name}.`, line);
        }
        return value;
      }
      case "write": {
        return await this.write(e, line);
      }
      default: {
        return e satisfies never;
      }
    }
  }

  private async write(
    e: Extract<Expr, { kind: "write" }>,
    line: number
  ): Promise<string> {
    const value = e.using ? await this.evaluate(e.using, line) : undefined;
    const args =
      value === undefined ? { prompt: e.prompt } : { prompt: e.prompt, value };
    const record = await this.effect(
      "write",
      args,
      () => this.provider.write(e.prompt, value),
      false
    );
    if (
      typeof record.result !== "string" ||
      record.result.length > MAX_WRITE_CHARS
    ) {
      throw new RuntimeError("Writer returned invalid or oversized text.");
    }
    this.trace.push({ kind: "write", line, text: record.result });
    return record.result;
  }

  private async choose(
    value: Value,
    labels: readonly string[],
    line: number,
    chaos: boolean,
    threshold: number
  ): Promise<string | null> {
    const record = await this.effect(
      "judge",
      { value, labels },
      () => this.provider.judge(value, labels),
      chaos
    );
    const probabilities = distribution(record.result, labels);
    const p = (label: string): number => probabilities[label] ?? 0;
    const best = argmax(labels, p);
    const uncertain = p(best) < threshold;
    const chosen =
      chaos && !uncertain ? sampleLabel(labels, p, record.draw ?? 0) : best;
    this.trace.push({
      kind: "judge",
      line,
      detail: {
        value,
        labels,
        probabilities,
        chosen: uncertain ? null : chosen,
        threshold,
      },
    });
    return uncertain ? null : chosen;
  }

  private async execute(
    body: readonly Statement[],
    chaos: boolean,
    nested: boolean
  ): Promise<void> {
    if (nested) {
      this.scopes.unshift(new Map());
    }
    try {
      for (const s of body) {
        this.steps++;
        if (this.steps > MAX_STATEMENTS) {
          throw new RuntimeError(
            `Run stopped at the ${MAX_STATEMENTS} statement limit.`
          );
        }
        await this.statement(s, chaos);
      }
    } finally {
      if (nested) {
        this.scopes.shift();
      }
    }
  }

  private async statement(s: Statement, chaos: boolean): Promise<void> {
    switch (s.kind) {
      case "let":
      case "set": {
        await this.assign(s);
        return;
      }
      case "print": {
        const value = String(await this.evaluate(s.value, s.line));
        this.output.push(value);
        this.trace.push({ kind: "print", line: s.line, text: value });
        return;
      }
      case "if": {
        const value = await this.evaluate(s.value, s.line);
        const no = `NOT: ${s.question}`;
        const selected = await this.choose(
          value,
          [s.question, no],
          s.line,
          chaos,
          s.confidence
        );
        await this.execute(ifBranch(s, selected), chaos, true);
        return;
      }
      case "match": {
        const value = await this.evaluate(s.value, s.line);
        const selected = await this.choose(
          value,
          s.branches.map((b) => b.label),
          s.line,
          chaos,
          0
        );
        const branch = s.branches.find((b) => b.label === selected);
        if (branch === undefined) {
          throw new RuntimeError("Match selected an unknown label.");
        }
        await this.execute(branch.body, chaos, true);
        return;
      }
      case "while": {
        await this.whileLoop(s, chaos);
        return;
      }
      case "repeat": {
        for (let i = 0; i < s.count; i++) {
          this.trace.push({
            kind: "repeat",
            line: s.line,
            text: `Iteration ${i + 1} of ${s.count}`,
          });
          await this.execute(s.body, chaos, true);
        }
        return;
      }
      case "chaos": {
        await this.execute(s.body, true, true);
        return;
      }
      default: {
        s satisfies never;
      }
    }
  }

  private async assign(
    s: Extract<Statement, { kind: "let" | "set" }>
  ): Promise<void> {
    const scope = s.kind === "let" ? this.scopes[0] : this.scopeOf(s.name);
    if (scope === undefined) {
      throw new LanguageError(
        `Unknown variable ${s.name}. Use let first.`,
        s.line
      );
    }
    if (s.kind === "let" && scope.has(s.name)) {
      throw new LanguageError(
        `${s.name} is already declared in this block.`,
        s.line
      );
    }
    const value = await this.evaluate(s.value, s.line);
    scope.set(s.name, value);
    this.trace.push({ kind: "assign", line: s.line, name: s.name, value });
  }

  private async whileLoop(
    s: Extract<Statement, { kind: "while" }>,
    chaos: boolean
  ): Promise<void> {
    let i = 0;
    while (true) {
      const value = await this.evaluate(s.value, s.line);
      const selected = await this.choose(
        value,
        [s.question, `NOT: ${s.question}`],
        s.line,
        chaos,
        0.5
      );
      if (selected !== s.question) {
        return;
      }
      if (i === MAX_WHILE_ITERATIONS) {
        throw new LanguageError(
          `Loop still feels true after ${MAX_WHILE_ITERATIONS} iterations.`,
          s.line
        );
      }
      i++;
      this.trace.push({
        kind: "repeat",
        line: s.line,
        text: `Iteration ${i} of at most ${MAX_WHILE_ITERATIONS}`,
      });
      await this.execute(s.body, chaos, true);
    }
  }
}

export async function run(
  source: string,
  provider: Provider,
  options: RunOptions = {}
): Promise<Run> {
  const input = options.input ?? "";
  const maxInputChars = options.maxInputChars ?? DEFAULT_MAX_INPUT_CHARS;
  if (input.length > maxInputChars) {
    throw new RuntimeError(`Input exceeds ${maxInputChars} characters.`);
  }
  return await new Interpreter(provider, input, options).run(source);
}
