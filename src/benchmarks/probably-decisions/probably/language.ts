export type Value = string | number | boolean;

type Token = {
  readonly text: string;
  readonly kind: "word" | "string" | "number" | "symbol" | "end";
  readonly line: number;
};

export type Expr =
  | { readonly kind: "literal"; readonly value: Value }
  | { readonly kind: "variable"; readonly name: string }
  | { readonly kind: "input" }
  | { readonly kind: "write"; readonly prompt: string; readonly using?: Expr };

type Base = { readonly line: number };

export type MatchBranch = {
  readonly label: string;
  readonly body: readonly Statement[];
};

export type Statement = Base &
  (
    | {
        readonly kind: "let" | "set";
        readonly name: string;
        readonly value: Expr;
      }
    | { readonly kind: "print"; readonly value: Expr }
    | {
        readonly kind: "if";
        readonly value: Expr;
        readonly question: string;
        readonly confidence: number;
        readonly yes: readonly Statement[];
        readonly maybe: readonly Statement[];
        readonly no: readonly Statement[];
      }
    | {
        readonly kind: "match";
        readonly value: Expr;
        readonly branches: readonly MatchBranch[];
      }
    | {
        readonly kind: "while";
        readonly value: Expr;
        readonly question: string;
        readonly body: readonly Statement[];
      }
    | {
        readonly kind: "repeat";
        readonly count: number;
        readonly body: readonly Statement[];
      }
    | { readonly kind: "chaos"; readonly body: readonly Statement[] }
  );

export class LanguageError extends Error {
  readonly line: number;

  constructor(message: string, line: number) {
    super(`Line ${line}: ${message}`);
    this.name = "LanguageError";
    this.line = line;
  }
}

const MAX_SOURCE_CHARS = 12_000;
const MAX_NESTING = 12;
const WORD = /^[A-Za-z_][A-Za-z_0-9]*/;
const NUMBER = /^\d+(?:\.\d+)?/;
const WHITESPACE = /\s/;

function lexString(
  source: string,
  start: number,
  line: number
): { token: Token; next: number } {
  let i = start + 1;
  while (i < source.length && source[i] !== '"') {
    if (source[i] === "\n") {
      throw new LanguageError("Use \\n inside strings.", line);
    }
    if (source[i] === "\\") {
      i++;
    }
    i++;
  }
  if (i >= source.length) {
    throw new LanguageError("Unterminated string.", line);
  }
  const raw = source.slice(start, i + 1);
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    throw new LanguageError("Invalid string escape.", line);
  }
  if (typeof value !== "string") {
    throw new LanguageError("Invalid string literal.", line);
  }
  return { token: { text: value, kind: "string", line }, next: i + 1 };
}

function lex(source: string): Token[] {
  if (source.length > MAX_SOURCE_CHARS) {
    throw new LanguageError(
      `Program exceeds ${MAX_SOURCE_CHARS} characters.`,
      1
    );
  }
  const tokens: Token[] = [];
  let i = 0;
  let line = 1;
  while (i < source.length) {
    const c = source[i] ?? "";
    if (WHITESPACE.test(c)) {
      if (c === "\n") {
        line++;
      }
      i++;
      continue;
    }
    if (source.startsWith("//", i)) {
      while (i < source.length && source[i] !== "\n") {
        i++;
      }
      continue;
    }
    if (c === '"') {
      const { token, next } = lexString(source, i, line);
      tokens.push(token);
      i = next;
      continue;
    }
    const rest = source.slice(i);
    const word = WORD.exec(rest);
    if (word) {
      tokens.push({ text: word[0], kind: "word", line });
      i += word[0].length;
      continue;
    }
    const number = NUMBER.exec(rest);
    if (number) {
      tokens.push({ text: number[0], kind: "number", line });
      i += number[0].length;
      continue;
    }
    if (source.startsWith("=>", i)) {
      tokens.push({ text: "=>", kind: "symbol", line });
      i += 2;
      continue;
    }
    if ("{}()=%;".includes(c)) {
      tokens.push({ text: c, kind: "symbol", line });
      i++;
      continue;
    }
    throw new LanguageError(`Unexpected character ${JSON.stringify(c)}.`, line);
  }
  return [...tokens, { text: "<end>", kind: "end", line }];
}

const RESERVED = new Set([
  "let",
  "print",
  "if",
  "feels",
  "with",
  "confidence",
  "otherwise",
  "maybe",
  "else",
  "match",
  "repeat",
  "while",
  "chaos",
  "llm",
  "write",
  "using",
  "input",
  "true",
  "false",
]);

class Parser {
  private readonly tokens: readonly Token[];
  private readonly end: Token;
  private index = 0;
  private depth = 0;

  constructor(source: string) {
    this.tokens = lex(source);
    this.end = this.tokens.at(-1) ?? { text: "<end>", kind: "end", line: 1 };
  }

  program(): Statement[] {
    const body = this.statements();
    if (this.peek().kind !== "end") {
      throw new LanguageError("Unexpected closing brace.", this.peek().line);
    }
    return body;
  }

  private peek(): Token {
    return this.tokens[this.index] ?? this.end;
  }

  private is(text: string): boolean {
    const t = this.peek();
    return t.text === text && t.kind !== "string";
  }

  private take(): Token {
    const t = this.tokens[this.index] ?? this.end;
    this.index++;
    return t;
  }

  private accept(text: string): boolean {
    if (this.is(text)) {
      this.take();
      return true;
    }
    return false;
  }

  private need(text: string): void {
    if (!this.accept(text)) {
      throw new LanguageError(
        `Expected ${text}, got ${this.peek().text}.`,
        this.peek().line
      );
    }
  }

  private str(): string {
    const t = this.take();
    if (t.kind !== "string") {
      throw new LanguageError("Expected a quoted string.", t.line);
    }
    return t.text;
  }

  private name(): string {
    const t = this.take();
    if (t.kind !== "word" || RESERVED.has(t.text)) {
      throw new LanguageError("Expected a variable name.", t.line);
    }
    return t.text;
  }

  private expr(): Expr {
    const t = this.peek();
    if (t.kind === "string") {
      this.take();
      return { kind: "literal", value: t.text };
    }
    if (t.kind === "number") {
      this.take();
      return { kind: "literal", value: Number(t.text) };
    }
    if (this.accept("true")) {
      return { kind: "literal", value: true };
    }
    if (this.accept("false")) {
      return { kind: "literal", value: false };
    }
    if (this.accept("input")) {
      this.need("(");
      this.need(")");
      return { kind: "input" };
    }
    if (this.accept("llm") || this.accept("write")) {
      const prompt = this.str();
      if (this.accept("using")) {
        return { kind: "write", prompt, using: this.atom() };
      }
      return { kind: "write", prompt };
    }
    return { kind: "variable", name: this.name() };
  }

  private atom(): Expr {
    if (this.is("llm") || this.is("write")) {
      throw new LanguageError(
        "Assign generated text before using it.",
        this.peek().line
      );
    }
    return this.expr();
  }

  private block(): Statement[] {
    this.need("{");
    this.depth++;
    if (this.depth > MAX_NESTING) {
      throw new LanguageError(
        `Nesting exceeds ${MAX_NESTING} blocks.`,
        this.peek().line
      );
    }
    const body = this.statements();
    this.need("}");
    this.depth--;
    return body;
  }

  private ifStatement(line: number): Statement {
    const value = this.atom();
    this.need("feels");
    const question = this.str();
    let confidence = 0.5;
    if (this.accept("with")) {
      this.need("confidence");
      const t = this.take();
      confidence = Number(t.text) / 100;
      if (t.kind !== "number" || confidence < 0.5 || confidence > 1) {
        throw new LanguageError("Confidence must be 50-100%.", t.line);
      }
      this.need("%");
    }
    const yes = this.block();
    let maybe: Statement[] = [];
    let no: Statement[] = [];
    if (this.accept("otherwise")) {
      this.need("maybe");
      maybe = this.block();
    }
    if (this.accept("else")) {
      no = this.block();
    }
    return { kind: "if", line, value, question, confidence, yes, maybe, no };
  }

  private matchStatement(line: number): Statement {
    const value = this.atom();
    this.need("{");
    const branches: MatchBranch[] = [];
    while (!this.is("}") && this.peek().kind !== "end") {
      const label = this.str();
      this.need("=>");
      branches.push({ label, body: this.block() });
    }
    this.need("}");
    const distinct = new Set(branches.map((b) => b.label)).size;
    if (
      branches.length < 2 ||
      branches.length > 8 ||
      distinct !== branches.length
    ) {
      throw new LanguageError("Match needs 2-8 distinct labels.", line);
    }
    return { kind: "match", line, value, branches };
  }

  private repeatStatement(line: number): Statement {
    const t = this.take();
    const count = Number(t.text);
    if (
      t.kind !== "number" ||
      !Number.isInteger(count) ||
      count < 1 ||
      count > 5
    ) {
      throw new LanguageError("Repeat needs an integer from 1 to 5.", t.line);
    }
    return { kind: "repeat", line, count, body: this.block() };
  }

  private statement(line: number): Statement {
    if (this.accept("let")) {
      const n = this.name();
      this.need("=");
      return { kind: "let", line, name: n, value: this.expr() };
    }
    if (this.accept("print")) {
      this.need("(");
      const value = this.expr();
      this.need(")");
      return { kind: "print", line, value };
    }
    if (this.accept("if")) {
      return this.ifStatement(line);
    }
    if (this.accept("match")) {
      return this.matchStatement(line);
    }
    if (this.accept("while")) {
      const value = this.atom();
      this.need("feels");
      const question = this.str();
      return { kind: "while", line, value, question, body: this.block() };
    }
    if (this.accept("repeat")) {
      return this.repeatStatement(line);
    }
    if (this.accept("chaos")) {
      return { kind: "chaos", line, body: this.block() };
    }
    const n = this.name();
    this.need("=");
    return { kind: "set", line, name: n, value: this.expr() };
  }

  private statements(): Statement[] {
    const out: Statement[] = [];
    while (this.peek().kind !== "end" && !this.is("}")) {
      if (this.accept(";")) {
        continue;
      }
      out.push(this.statement(this.peek().line));
    }
    return out;
  }
}

export function parse(source: string): Statement[] {
  return new Parser(source).program();
}
