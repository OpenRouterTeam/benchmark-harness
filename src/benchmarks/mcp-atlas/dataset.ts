import { map as mapEffect } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect as layerEffect, provide as layerProvide } from "effect/Layer";
import * as Result from "effect/Result";
import type { Stream } from "effect/Stream";
import {
  drop as streamDrop,
  filter as streamFilter,
  runCount,
  take as streamTake,
} from "effect/Stream";

import { makeHfDatasetLayer } from "../../datasets/huggingface";
import type { DatasetError, Sample } from "../../harness/core";
import type { DatasetStreamOptions } from "../../harness/dataset";
import { Dataset } from "../../harness/dataset";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import type { RetryConfig } from "../../runtime/retry";
import { MCP_ATLAS_ID } from "./schema";

export const MCP_ATLAS_DATASET = {
  dataset: "ScaleAI/MCP-Atlas",
  config: "default",
  split: "train",
} as const;

export const MCP_ATLAS_TASK_METADATA_KEY = "task";

export const McpAtlasTaskSchema = z.object({
  taskId: z.string(),
  enabledTools: z.array(z.string()).readonly(),
  claims: z.array(z.string()).readonly(),
});

export type McpAtlasTask = z.infer<typeof McpAtlasTaskSchema>;

export interface McpAtlasDatasetOptions {
  readonly serverAllowlist?: readonly string[];
}

const PYTHON_ESCAPES: Readonly<Record<string, string>> = {
  "'": "'",
  '"': '"',
  "\\": "\\",
  n: "\n",
  r: "\r",
  t: "\t",
};

function recordError(field: string, detail: string): Error {
  return new Error(`MCP-Atlas record field ${field}: ${detail}`);
}

function asString(
  record: Readonly<Record<string, unknown>>,
  field: string
): string {
  const value = record[field];
  if (typeof value !== "string") {
    throw recordError(field, `expected a string, got ${typeof value}`);
  }
  return value;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function listItemName(item: unknown, field: string): string {
  if (typeof item === "string") {
    return item;
  }
  if (isRecord(item) && typeof item["name"] === "string") {
    return item["name"];
  }
  throw recordError(field, "list item is neither a string nor a named tool");
}

function tryJsonParse(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function readEscape(
  text: string,
  index: number
): { readonly value: string; readonly next: number } {
  const escaped = text[index];
  if (escaped === undefined) {
    return { value: "", next: index };
  }
  if (escaped === "x" || escaped === "u") {
    const width = escaped === "x" ? 2 : 4;
    const hex = text.slice(index + 1, index + 1 + width);
    return {
      value: String.fromCodePoint(Number.parseInt(hex, 16)),
      next: index + 1 + width,
    };
  }
  return { value: PYTHON_ESCAPES[escaped] ?? escaped, next: index + 1 };
}

function readPythonString(
  text: string,
  start: number,
  quote: string,
  field: string
): { readonly value: string; readonly next: number } {
  let value = "";
  let index = start + 1;
  while (index < text.length) {
    const char = text[index];
    if (char === "\\") {
      const escape = readEscape(text, index + 1);
      value += escape.value;
      index = escape.next;
      continue;
    }
    if (char === quote) {
      return { value, next: index + 1 };
    }
    value += char;
    index += 1;
  }
  throw recordError(field, `unterminated string starting at ${start}`);
}

export function parsePythonStringList(
  text: string,
  field: string
): readonly string[] {
  if (!(text.startsWith("[") && text.endsWith("]"))) {
    throw recordError(field, "expected a bracketed list");
  }
  const items: string[] = [];
  const end = text.length - 1;
  let index = 1;
  while (index < end) {
    const char = text[index];
    if (char === undefined) {
      break;
    }
    if (char === " " || char === "," || char === "\n") {
      index += 1;
      continue;
    }
    if (char !== "'" && char !== '"') {
      throw recordError(field, `unexpected character ${char} at ${index}`);
    }
    const parsed = readPythonString(text, index, char, field);
    items.push(parsed.value);
    index = parsed.next;
  }
  return items;
}

export function parseStringList(
  value: unknown,
  field: string
): readonly string[] {
  if (Array.isArray(value)) {
    return value.map((item) => listItemName(item, field));
  }
  if (typeof value !== "string") {
    throw recordError(field, `expected a list, got ${typeof value}`);
  }
  const trimmed = value.trim();
  const json = tryJsonParse(trimmed);
  if (Array.isArray(json)) {
    return json.map((item) => listItemName(item, field));
  }
  return parsePythonStringList(trimmed, field);
}

export function toolServerName(
  toolName: string,
  servers: readonly string[]
): string | undefined {
  return [...servers]
    .sort((a, b) => b.length - a.length)
    .find((server) => toolName.startsWith(`${server}_`));
}

export function usesOnlyServers(
  toolNames: readonly string[],
  servers: readonly string[]
): boolean {
  return toolNames.every(
    (toolName) => toolServerName(toolName, servers) !== undefined
  );
}

export function recordToSample(
  record: Readonly<Record<string, unknown>>,
  index: number
): Sample {
  const task: McpAtlasTask = {
    taskId: asString(record, "TASK"),
    enabledTools: parseStringList(record["ENABLED_TOOLS"], "ENABLED_TOOLS"),
    claims: parseStringList(record["GTFA_CLAIMS"], "GTFA_CLAIMS"),
  };
  return {
    id: `${MCP_ATLAS_ID}-${index}`,
    input: asString(record, "PROMPT"),
    target: { text: task.claims.join("\n") },
    metadata: { [MCP_ATLAS_TASK_METADATA_KEY]: task },
  };
}

export function sampleTask(sample: Sample): McpAtlasTask | undefined {
  const parsed = parseSchema(
    McpAtlasTaskSchema,
    sample.metadata?.[MCP_ATLAS_TASK_METADATA_KEY]
  );
  return Result.isSuccess(parsed) ? parsed.success : undefined;
}

function sliceStream(
  source: Stream<Sample, DatasetError>,
  opts: DatasetStreamOptions | undefined
): Stream<Sample, DatasetError> {
  const start = opts?.start ?? 0;
  const dropped = start > 0 ? source.pipe(streamDrop(start)) : source;
  return opts?.end === undefined
    ? dropped
    : dropped.pipe(streamTake(Math.max(0, opts.end - start)));
}

export function keepSamplesOnServers(
  layer: Layer<Dataset>,
  servers: readonly string[]
): Layer<Dataset> {
  const isAllowed = (sample: Sample): boolean => {
    const task = sampleTask(sample);
    return task !== undefined && usesOnlyServers(task.enabledTools, servers);
  };
  return layerEffect(
    Dataset,
    mapEffect(Dataset, (dataset) => {
      const filtered = dataset.stream().pipe(streamFilter(isAllowed));
      const stream: Dataset["Service"]["stream"] = (opts) =>
        sliceStream(filtered, opts);
      return { size: runCount(filtered), stream };
    })
  ).pipe(layerProvide(layer));
}

export function makeMcpAtlasDatasetLayer(
  retryConfig?: RetryConfig,
  options?: McpAtlasDatasetOptions
): Layer<Dataset> {
  const layer = makeHfDatasetLayer({
    ...MCP_ATLAS_DATASET,
    recordToSample,
    ...definedValues({ retry: retryConfig }),
  });
  const servers = options?.serverAllowlist;
  return servers === undefined ? layer : keepSamplesOnServers(layer, servers);
}
