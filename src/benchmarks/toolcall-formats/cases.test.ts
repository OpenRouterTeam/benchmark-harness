import { describe, expect, it } from "bun:test";
import assert from "node:assert/strict";

import {
  getToolCallFormatCase,
  OptionalEncoding,
  SCHEMA_VARIANTS,
  TOOLCALL_FORMAT_CASES,
} from "./cases";

function readParameters(id: string): Readonly<Record<string, unknown>> {
  const entry = getToolCallFormatCase(id);
  const tool = entry?.tools.find(
    (candidate) => candidate.function.name === "read"
  );
  assert(tool !== undefined, `missing read tool on ${id}`);
  return tool.function.parameters;
}

describe("toolcall_formats cases", () => {
  it("crosses every scenario with every schema variant under unique ids", () => {
    const ids = TOOLCALL_FORMAT_CASES.map((entry) => entry.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids.length % SCHEMA_VARIANTS.length).toBe(0);
    expect(SCHEMA_VARIANTS.map((variant) => variant.encoding)).toContain(
      OptionalEncoding.AnyOfNull
    );
  });

  it("leaves optional fields out of required in the omitted encoding", () => {
    expect(readParameters("toolcall_formats-read_slice-omitted")).toEqual({
      type: "object",
      required: ["path"],
      properties: {
        path: { type: "string", description: "Path to the file" },
        offset: {
          type: "number",
          description: "Line number to start reading from (1-indexed)",
        },
        limit: {
          type: "number",
          description: "Maximum number of lines to read",
        },
      },
    });
  });

  it("matches pi's strict rewrite for anyof_null_strict", () => {
    const parameters = readParameters(
      "toolcall_formats-read_slice-anyof_null_strict"
    );
    expect(parameters).toMatchObject({
      required: ["path", "offset", "limit"],
      additionalProperties: false,
      properties: {
        offset: { anyOf: [{ type: "number" }, { type: "null" }] },
      },
    });
    const entry = getToolCallFormatCase(
      "toolcall_formats-read_slice-anyof_null_strict"
    );
    expect(entry?.tools[0]?.function.strict).toBe(true);
  });

  it("adds null to enums in the type-array encoding and closes nested objects", () => {
    const entry = getToolCallFormatCase(
      "toolcall_formats-weather_negative-type_array_null_strict"
    );
    expect(entry?.tools[0]?.function.parameters).toMatchObject({
      properties: {
        units: { type: ["string", "null"], enum: ["metric", "imperial", null] },
      },
    });
    const meeting = getToolCallFormatCase(
      "toolcall_formats-meeting_nested-anyof_null_strict"
    );
    expect(meeting?.tools[0]?.function.parameters).toMatchObject({
      properties: {
        start: { additionalProperties: false },
        attendees: { items: { additionalProperties: false } },
        location: {
          anyOf: [{ additionalProperties: false }, { type: "null" }],
        },
      },
    });
  });
});
