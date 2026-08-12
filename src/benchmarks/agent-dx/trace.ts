import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import type { BenchmarkResultRow } from "../../results/parquet-schema";
import { parseSubcheckLines } from "./quality";

export type TraceEvent =
  | { readonly kind: "text"; readonly text: string }
  | { readonly kind: "reasoning"; readonly text: string }
  | {
      readonly kind: "tool";
      readonly tool: string;
      readonly input: string;
      readonly outputPreview: string;
      readonly errored?: boolean;
    };

export type Subchecks = Readonly<Record<string, boolean>>;

export interface ResourceUsage {
  readonly mcpToolCalls: number;
  readonly skillInvocations: number;
  readonly docsReads: number;
  readonly webFetches: number;
}

export interface FrictionDiagnostics {
  readonly toolCalls: number;
  readonly erroredToolCalls: number;
  readonly appRunRetries: number;
}

export interface TrialTrace {
  readonly taskId: string;
  readonly epoch: number;
  readonly passed: boolean;
  readonly events: readonly TraceEvent[];
  readonly subchecks: Subchecks;
  readonly resourceUsage: ResourceUsage;
  readonly friction: FrictionDiagnostics;
  readonly verifierOutput: string;
}

export function parseSubchecks(verifierOutput: string): Subchecks {
  return Object.fromEntries(
    parseSubcheckLines(verifierOutput).map((outcome) => [
      outcome.name,
      outcome.passed,
    ])
  );
}

const DOCS_SNAPSHOT_DIR = "/opt/openrouter-docs";

const SHELL_TOOLS = new Set(["bash", "Bash", "shell"]);
const SHELL_WEB_FETCH_PATTERN = /\b(?:curl|wget)\b[^\n]*https?:\/\//;

export function resourceUsageFromEvents(
  events: readonly TraceEvent[]
): ResourceUsage {
  let mcpToolCalls = 0;
  let skillInvocations = 0;
  let docsReads = 0;
  let webFetches = 0;
  for (const event of events) {
    if (event.kind !== "tool") {
      continue;
    }
    if (
      event.tool.startsWith("openrouter") ||
      event.tool.startsWith("mcp__openrouter")
    ) {
      mcpToolCalls += 1;
    } else if (
      event.tool === "skill" ||
      event.tool === "Skill" ||
      ((event.tool === "read" || event.tool === "Read") &&
        event.input.includes("SKILL.md"))
    ) {
      skillInvocations += 1;
    } else if (
      event.tool === "webfetch" ||
      event.tool === "WebFetch" ||
      (SHELL_TOOLS.has(event.tool) && SHELL_WEB_FETCH_PATTERN.test(event.input))
    ) {
      webFetches += 1;
    } else if (event.input.includes(DOCS_SNAPSHOT_DIR)) {
      docsReads += 1;
    }
  }
  return { mcpToolCalls, skillInvocations, docsReads, webFetches };
}

const APP_RUN_PATTERN =
  /\b(?:npm|bun|pnpm|yarn)\s+(?:run\s+\S+|start|test)\b|\bnode\s+\S|\btsc\b/;

export function frictionFromEvents(
  events: readonly TraceEvent[]
): FrictionDiagnostics {
  const toolEvents = events.filter((event) => event.kind === "tool");
  const appRuns = toolEvents.filter(
    (event) => SHELL_TOOLS.has(event.tool) && APP_RUN_PATTERN.test(event.input)
  ).length;
  return {
    toolCalls: toolEvents.length,
    erroredToolCalls: toolEvents.filter((event) => event.errored === true)
      .length,
    appRunRetries: Math.max(0, appRuns - 1),
  };
}

export function frictionFromMessages(
  messages: string | null
): FrictionDiagnostics {
  return frictionFromEvents(
    parseTraceEvents(agentEventStreamFromMessages(messages))
  );
}

export function agentEventStreamFromMessages(messages: string | null): string {
  if (messages === null) {
    return "";
  }
  const parsed = Either.try(() => JSON.parse(messages));
  if (Either.isLeft(parsed) || !Array.isArray(parsed.right)) {
    return "";
  }
  const assistant = parsed.right.find(
    (message: unknown) => isRecord(message) && message["role"] === "assistant"
  );
  return isRecord(assistant) && typeof assistant["content"] === "string"
    ? assistant["content"]
    : "";
}

export function resourceUsageFromMessages(
  messages: string | null
): ResourceUsage {
  return resourceUsageFromEvents(
    parseTraceEvents(agentEventStreamFromMessages(messages))
  );
}

export function parseTraceEvents(eventStream: string): TraceEvent[] {
  const events: TraceEvent[] = [];
  const toolEventIndexById = new Map<string, number>();
  for (const line of eventStream.split("\n")) {
    const trimmed = line.trim();
    if (trimmed === "") {
      continue;
    }
    const parsed = Either.try(() => JSON.parse(trimmed));
    if (Either.isLeft(parsed) || !isRecord(parsed.right)) {
      continue;
    }
    const event = parsed.right;
    const part = isRecord(event["part"]) ? event["part"] : {};
    if (event["type"] === "text" || event["type"] === "reasoning") {
      const text = typeof part["text"] === "string" ? part["text"] : "";
      if (text !== "") {
        events.push({
          kind: event["type"] === "text" ? "text" : "reasoning",
          text,
        });
      }
    } else if (event["type"] === "tool_use") {
      const source = typeof part["tool"] === "string" ? part : event;
      const state = isRecord(source["state"]) ? source["state"] : {};
      events.push({
        kind: "tool",
        tool: typeof source["tool"] === "string" ? source["tool"] : "unknown",
        input: JSON.stringify(state["input"] ?? {}),
        outputPreview:
          typeof state["output"] === "string"
            ? state["output"].slice(0, 400)
            : "",
        errored:
          state["status"] === "error" || typeof state["error"] === "string",
      });
    } else if (event["type"] === "assistant") {
      for (const { traceEvent, toolUseId } of claudeContentBlockEvents(
        event["message"]
      )) {
        if (toolUseId !== undefined) {
          toolEventIndexById.set(toolUseId, events.length);
        }
        events.push(traceEvent);
      }
    } else if (event["type"] === "user") {
      applyClaudeToolResults(event["message"], events, toolEventIndexById);
    }
  }
  return events;
}

interface ClaudeBlockEvent {
  readonly traceEvent: TraceEvent;
  readonly toolUseId?: string;
}

function claudeContentBlockEvents(message: unknown): ClaudeBlockEvent[] {
  if (!isRecord(message) || !Array.isArray(message["content"])) {
    return [];
  }
  const events: ClaudeBlockEvent[] = [];
  for (const block of message["content"]) {
    if (!isRecord(block)) {
      continue;
    }
    if (
      block["type"] === "text" &&
      typeof block["text"] === "string" &&
      block["text"] !== ""
    ) {
      events.push({ traceEvent: { kind: "text", text: block["text"] } });
    } else if (
      block["type"] === "thinking" &&
      typeof block["thinking"] === "string" &&
      block["thinking"] !== ""
    ) {
      events.push({
        traceEvent: { kind: "reasoning", text: block["thinking"] },
      });
    } else if (block["type"] === "tool_use") {
      events.push({
        traceEvent: {
          kind: "tool",
          tool: typeof block["name"] === "string" ? block["name"] : "unknown",
          input: JSON.stringify(block["input"] ?? {}),
          outputPreview: "",
        },
        ...(typeof block["id"] === "string" && { toolUseId: block["id"] }),
      });
    }
  }
  return events;
}

function applyClaudeToolResults(
  message: unknown,
  events: TraceEvent[],
  toolEventIndexById: ReadonlyMap<string, number>
): void {
  if (!isRecord(message) || !Array.isArray(message["content"])) {
    return;
  }
  for (const block of message["content"]) {
    if (
      !isRecord(block) ||
      block["type"] !== "tool_result" ||
      typeof block["tool_use_id"] !== "string"
    ) {
      continue;
    }
    const index = toolEventIndexById.get(block["tool_use_id"]);
    const existing = index === undefined ? undefined : events[index];
    if (index === undefined || existing?.kind !== "tool") {
      continue;
    }
    events[index] = {
      ...existing,
      errored: block["is_error"] === true,
      outputPreview:
        existing.outputPreview === ""
          ? claudeToolResultPreview(block["content"])
          : existing.outputPreview,
    };
  }
}

function claudeToolResultPreview(content: unknown): string {
  if (typeof content === "string") {
    return content.slice(0, 400);
  }
  if (!Array.isArray(content)) {
    return "";
  }
  const text = content
    .filter(
      (block): block is Record<string, unknown> =>
        isRecord(block) && typeof block["text"] === "string"
    )
    .map((block) => block["text"])
    .join("\n");
  return text.slice(0, 400);
}

export function tracesFromResultRows(
  rows: readonly BenchmarkResultRow[]
): TrialTrace[] {
  return rows.map((row) => {
    const verifierOutput = row.explanation ?? "";
    const events = parseTraceEvents(agentEventStreamFromMessages(row.messages));
    return {
      taskId: row.sample_id,
      epoch: row.epoch,
      passed: row.score_value === "C",
      events,
      subchecks: parseSubchecks(verifierOutput),
      resourceUsage: resourceUsageFromEvents(events),
      friction: frictionFromEvents(events),
      verifierOutput,
    };
  });
}

const MAX_TEXT_PREVIEW = 300;

export function formatTrialTrace(trace: TrialTrace): string {
  const lines: string[] = [
    `## ${trace.taskId} (epoch ${trace.epoch}) — ${trace.passed ? "PASS" : "FAIL"}`,
  ];
  const subcheckEntries = Object.entries(trace.subchecks);
  if (subcheckEntries.length > 0) {
    lines.push(
      `subchecks: ${subcheckEntries.map(([name, ok]) => `${name}=${ok ? "pass" : "fail"}`).join(" ")}`
    );
  }
  const usage = trace.resourceUsage;
  lines.push(
    `resources: mcp=${usage.mcpToolCalls} skills=${usage.skillInvocations} docs=${usage.docsReads} webfetch=${usage.webFetches}`
  );
  const friction = trace.friction;
  lines.push(
    `friction: tools=${friction.toolCalls} errored=${friction.erroredToolCalls} reruns=${friction.appRunRetries}`
  );
  lines.push("");
  for (const [index, event] of trace.events.entries()) {
    switch (event.kind) {
      case "text":
      case "reasoning": {
        lines.push(
          `${index}. [${event.kind}] ${truncate(event.text, MAX_TEXT_PREVIEW)}`
        );
        break;
      }
      case "tool": {
        lines.push(
          `${index}. [tool:${event.tool}] ${truncate(event.input, MAX_TEXT_PREVIEW)}`
        );
        if (event.outputPreview !== "") {
          lines.push(
            `   -> ${truncate(event.outputPreview, MAX_TEXT_PREVIEW)}`
          );
        }
        break;
      }
      default: {
        event satisfies never;
      }
    }
  }
  if (!trace.passed && trace.verifierOutput !== "") {
    lines.push("", "### verifier output", truncate(trace.verifierOutput, 2000));
  }
  return lines.join("\n");
}

function truncate(text: string, max: number): string {
  const flattened = text.replaceAll("\n", " ");
  return flattened.length <= max ? flattened : `${flattened.slice(0, max)}…`;
}
