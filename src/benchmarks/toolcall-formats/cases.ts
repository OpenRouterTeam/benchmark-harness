import type { ToolDefinition } from "../../harness/core";
import type { ValueOf } from "../../internal/guards";
import { isRecord } from "../../internal/guards";
import type { JsonSchema } from "./json-schema";

export const OptionalEncoding = {
  Omitted: "omitted",
  AnyOfNull: "anyof_null",
  TypeArrayNull: "type_array_null",
} as const;

export type OptionalEncoding = ValueOf<typeof OptionalEncoding>;

export interface SchemaVariant {
  readonly encoding: OptionalEncoding;
  readonly strict: boolean;
}

export const SCHEMA_VARIANTS: readonly SchemaVariant[] = [
  { encoding: OptionalEncoding.Omitted, strict: false },
  { encoding: OptionalEncoding.AnyOfNull, strict: false },
  { encoding: OptionalEncoding.AnyOfNull, strict: true },
  { encoding: OptionalEncoding.TypeArrayNull, strict: false },
  { encoding: OptionalEncoding.TypeArrayNull, strict: true },
];

interface FieldSpec {
  readonly name: string;
  readonly description: string;
  readonly schema: JsonSchema;
  readonly optional?: boolean;
}

interface ToolSpec {
  readonly name: string;
  readonly description: string;
  readonly fields: readonly FieldSpec[];
}

export interface ExpectedCall {
  readonly tool: string;
  readonly args: Readonly<Record<string, unknown>>;
}

interface Scenario {
  readonly id: string;
  readonly tools: readonly ToolSpec[];
  readonly prompt: string;
  readonly calls: readonly ExpectedCall[];
}

export interface ToolCallFormatCase {
  readonly id: string;
  readonly scenarioId: string;
  readonly variant: SchemaVariant;
  readonly prompt: string;
  readonly tools: readonly ToolDefinition[];
  readonly calls: readonly ExpectedCall[];
}

const STRING = { type: "string" } as const;
const NUMBER = { type: "number" } as const;
const INTEGER = { type: "integer" } as const;
const BOOLEAN = { type: "boolean" } as const;

const READ_TOOL: ToolSpec = {
  name: "read",
  description:
    "Read the contents of a file. Use offset/limit to read a slice of lines.",
  fields: [
    { name: "path", description: "Path to the file", schema: STRING },
    {
      name: "offset",
      description: "Line number to start reading from (1-indexed)",
      schema: NUMBER,
      optional: true,
    },
    {
      name: "limit",
      description: "Maximum number of lines to read",
      schema: NUMBER,
      optional: true,
    },
  ],
};

const BASH_TOOL: ToolSpec = {
  name: "bash",
  description: "Execute a shell command.",
  fields: [
    { name: "command", description: "Command to execute", schema: STRING },
    {
      name: "timeout",
      description: "Timeout in seconds",
      schema: NUMBER,
      optional: true,
    },
  ],
};

const THERMOSTAT_TOOL: ToolSpec = {
  name: "set_thermostat",
  description: "Set the target temperature for a room.",
  fields: [
    {
      name: "room",
      description: "Room to control",
      schema: { type: "string", enum: ["kitchen", "bedroom", "office"] },
    },
    {
      name: "temperature_c",
      description: "Target temperature in whole degrees Celsius",
      schema: INTEGER,
    },
    {
      name: "fan_speed",
      description: "Fan speed from 1 to 5",
      schema: { type: "integer", minimum: 1, maximum: 5 },
      optional: true,
    },
  ],
};

const TICKET_TOOL: ToolSpec = {
  name: "create_ticket",
  description: "Create an issue tracker ticket.",
  fields: [
    { name: "title", description: "Ticket title", schema: STRING },
    {
      name: "priority",
      description: "Ticket priority",
      schema: { type: "string", enum: ["low", "medium", "high", "urgent"] },
    },
    {
      name: "blocking",
      description: "Whether the ticket blocks a release",
      schema: BOOLEAN,
      optional: true,
    },
    {
      name: "assignee",
      description: "Username of the assignee",
      schema: STRING,
      optional: true,
    },
  ],
};

const LABEL_TOOL: ToolSpec = {
  name: "label_issue",
  description: "Add labels to an issue.",
  fields: [
    { name: "issue_id", description: "Numeric issue id", schema: INTEGER },
    {
      name: "labels",
      description: "Labels to add",
      schema: { type: "array", items: STRING },
    },
    {
      name: "weights",
      description: "Optional per-label weights, same order as labels",
      schema: { type: "array", items: NUMBER },
      optional: true,
    },
  ],
};

const SCORES_TOOL: ToolSpec = {
  name: "record_scores",
  description: "Record evaluation scores for a run.",
  fields: [
    { name: "run_id", description: "Run identifier", schema: STRING },
    {
      name: "scores",
      description: "Scores to record",
      schema: { type: "array", items: NUMBER },
    },
    {
      name: "notes",
      description: "Free-form notes",
      schema: STRING,
      optional: true,
    },
  ],
};

const MEETING_TOOL: ToolSpec = {
  name: "schedule_meeting",
  description: "Schedule a calendar meeting.",
  fields: [
    { name: "title", description: "Meeting title", schema: STRING },
    {
      name: "start",
      description: "Start time",
      schema: {
        type: "object",
        properties: {
          date: { type: "string", description: "YYYY-MM-DD" },
          hour: { type: "integer", description: "0-23" },
          minute: { type: "integer", description: "0-59" },
        },
        required: ["date", "hour", "minute"],
      },
    },
    {
      name: "duration_minutes",
      description: "Duration in minutes",
      schema: INTEGER,
    },
    {
      name: "attendees",
      description: "People to invite",
      schema: {
        type: "array",
        items: {
          type: "object",
          properties: {
            email: STRING,
            optional: { type: "boolean", description: "Attendance optional" },
          },
          required: ["email", "optional"],
        },
      },
    },
    {
      name: "location",
      description: "Physical location",
      schema: {
        type: "object",
        properties: { room: STRING, floor: INTEGER },
        required: ["room", "floor"],
      },
      optional: true,
    },
  ],
};

const TRANSFER_TOOL: ToolSpec = {
  name: "transfer_funds",
  description: "Transfer money between accounts.",
  fields: [
    { name: "from_account", description: "Source account", schema: STRING },
    { name: "to_account", description: "Destination account", schema: STRING },
    { name: "amount", description: "Amount to transfer", schema: NUMBER },
    {
      name: "currency",
      description: "ISO currency code",
      schema: { type: "string", enum: ["USD", "EUR", "GBP"] },
    },
    { name: "memo", description: "Memo line", schema: STRING, optional: true },
  ],
};

const WEATHER_TOOL: ToolSpec = {
  name: "get_weather",
  description: "Get the weather forecast for a coordinate.",
  fields: [
    { name: "latitude", description: "Latitude", schema: NUMBER },
    { name: "longitude", description: "Longitude", schema: NUMBER },
    {
      name: "units",
      description: "Unit system",
      schema: { type: "string", enum: ["metric", "imperial"] },
      optional: true,
    },
    {
      name: "days",
      description: "Number of forecast days",
      schema: INTEGER,
      optional: true,
    },
  ],
};

const WRITE_TOOL: ToolSpec = {
  name: "write_file",
  description: "Write text content to a file.",
  fields: [
    { name: "path", description: "Path to the file", schema: STRING },
    {
      name: "content",
      description: "Exact file content",
      schema: STRING,
    },
    {
      name: "append",
      description: "Append instead of overwrite",
      schema: BOOLEAN,
      optional: true,
    },
  ],
};

const SCALE_TOOL: ToolSpec = {
  name: "scale_pool",
  description: "Scale a worker pool.",
  fields: [
    { name: "pool", description: "Pool name", schema: STRING },
    {
      name: "replicas",
      description: "Replica count",
      schema: { type: "integer", enum: [1, 2, 4, 8] },
    },
    {
      name: "dry_run",
      description: "Only report the plan",
      schema: BOOLEAN,
      optional: true,
    },
  ],
};

const LIST_ALERTS_TOOL: ToolSpec = {
  name: "list_active_alerts",
  description: "List currently firing alerts. Takes no arguments.",
  fields: [],
};

const SCENARIOS: readonly Scenario[] = [
  {
    id: "read_slice",
    tools: [READ_TOOL, BASH_TOOL],
    prompt: "Read lines 40-80 of src/server.ts",
    calls: [{ tool: "read", args: { path: "src/server.ts", offset: 40 } }],
  },
  {
    id: "read_whole",
    tools: [READ_TOOL, BASH_TOOL],
    prompt: "Read the whole file README.md. Do not pass an offset or limit.",
    calls: [
      { tool: "read", args: { path: "README.md", offset: null, limit: null } },
    ],
  },
  {
    id: "bash_timeout",
    tools: [READ_TOOL, BASH_TOOL],
    prompt: "Run `npm test` with a timeout of 120 seconds.",
    calls: [{ tool: "bash", args: { command: "npm test", timeout: 120 } }],
  },
  {
    id: "bash_plain",
    tools: [READ_TOOL, BASH_TOOL],
    prompt: "Run `git status` with no timeout.",
    calls: [{ tool: "bash", args: { command: "git status", timeout: null } }],
  },
  {
    id: "parallel_reads",
    tools: [READ_TOOL, BASH_TOOL],
    prompt:
      "Read package.json, tsconfig.json and README.md in full, without an offset or limit. Issue all three read calls at once.",
    calls: [
      {
        tool: "read",
        args: { path: "package.json", offset: null, limit: null },
      },
      {
        tool: "read",
        args: { path: "tsconfig.json", offset: null, limit: null },
      },
      { tool: "read", args: { path: "README.md", offset: null, limit: null } },
    ],
  },
  {
    id: "thermostat_full",
    tools: [THERMOSTAT_TOOL],
    prompt: "Set the office to 21 degrees Celsius with fan speed 3.",
    calls: [
      {
        tool: "set_thermostat",
        args: { room: "office", temperature_c: 21, fan_speed: 3 },
      },
    ],
  },
  {
    id: "thermostat_minimal",
    tools: [THERMOSTAT_TOOL],
    prompt: "Set the bedroom to 18 degrees Celsius. Leave the fan alone.",
    calls: [
      {
        tool: "set_thermostat",
        args: { room: "bedroom", temperature_c: 18, fan_speed: null },
      },
    ],
  },
  {
    id: "ticket_full",
    tools: [TICKET_TOOL],
    prompt:
      "File an urgent ticket titled 'Checkout returns 500' that blocks the release, assigned to dana.",
    calls: [
      {
        tool: "create_ticket",
        args: { priority: "urgent", blocking: true, assignee: "dana" },
      },
    ],
  },
  {
    id: "ticket_minimal",
    tools: [TICKET_TOOL],
    prompt:
      "File a low priority ticket titled 'Typo on pricing page'. Leave assignee and blocking unset.",
    calls: [
      {
        tool: "create_ticket",
        args: { priority: "low", blocking: null, assignee: null },
      },
    ],
  },
  {
    id: "label_array",
    tools: [LABEL_TOOL],
    prompt: "Add the labels bug, p1 and regression to issue 4821.",
    calls: [
      {
        tool: "label_issue",
        args: {
          issue_id: 4821,
          labels: ["bug", "p1", "regression"],
          weights: null,
        },
      },
    ],
  },
  {
    id: "label_weights",
    tools: [LABEL_TOOL],
    prompt:
      "Add labels perf and infra to issue 77 with weights 0.25 and 1.5 respectively.",
    calls: [
      {
        tool: "label_issue",
        args: { issue_id: 77, labels: ["perf", "infra"], weights: [0.25, 1.5] },
      },
    ],
  },
  {
    id: "scores_numbers",
    tools: [SCORES_TOOL],
    prompt: "Record scores 0.5, 1.25, -3 and 1e-4 for run r-17. No notes.",
    calls: [
      {
        tool: "record_scores",
        args: { run_id: "r-17", scores: [0.5, 1.25, -3, 0.0001], notes: null },
      },
    ],
  },
  {
    id: "meeting_nested",
    tools: [MEETING_TOOL],
    prompt:
      "Schedule 'Q3 planning' on 2026-10-05 at 14:30 for 45 minutes with alice@example.com (required) and bob@example.com (optional), in room Atlas on floor 7.",
    calls: [
      {
        tool: "schedule_meeting",
        args: {
          start: { date: "2026-10-05", hour: 14, minute: 30 },
          duration_minutes: 45,
          attendees: [
            { email: "alice@example.com", optional: false },
            { email: "bob@example.com", optional: true },
          ],
          location: { room: "Atlas", floor: 7 },
        },
      },
    ],
  },
  {
    id: "meeting_no_location",
    tools: [MEETING_TOOL],
    prompt:
      "Schedule a video call 'Sync' on 2026-11-02 at 09:05 for 15 minutes with carol@example.com (required). No physical location.",
    calls: [
      {
        tool: "schedule_meeting",
        args: {
          start: { date: "2026-11-02", hour: 9, minute: 5 },
          duration_minutes: 15,
          attendees: [{ email: "carol@example.com", optional: false }],
          location: null,
        },
      },
    ],
  },
  {
    id: "transfer_decimal",
    tools: [TRANSFER_TOOL],
    prompt: "Transfer 1250.75 EUR from ACC-001 to ACC-982. No memo.",
    calls: [
      {
        tool: "transfer_funds",
        args: {
          from_account: "ACC-001",
          to_account: "ACC-982",
          amount: 1250.75,
          currency: "EUR",
          memo: null,
        },
      },
    ],
  },
  {
    id: "weather_negative",
    tools: [WEATHER_TOOL],
    prompt:
      "Get the imperial-units forecast for the next 3 days at latitude -33.8688, longitude 151.2093.",
    calls: [
      {
        tool: "get_weather",
        args: {
          latitude: -33.8688,
          longitude: 151.2093,
          units: "imperial",
          days: 3,
        },
      },
    ],
  },
  {
    id: "parallel_weather",
    tools: [WEATHER_TOOL],
    prompt:
      "Get the default forecast for latitude 51.5072, longitude -0.1276 and for latitude 40.7128, longitude -74.006. Make both calls at once and leave units and days unset.",
    calls: [
      {
        tool: "get_weather",
        args: {
          latitude: 51.5072,
          longitude: -0.1276,
          units: null,
          days: null,
        },
      },
      {
        tool: "get_weather",
        args: {
          latitude: 40.7128,
          longitude: -74.006,
          units: null,
          days: null,
        },
      },
    ],
  },
  {
    id: "write_json_content",
    tools: [WRITE_TOOL],
    prompt:
      'Overwrite config/app.json with exactly this content: {"port": 8080, "debug": true, "tags": ["x", "y"]}',
    calls: [
      {
        tool: "write_file",
        args: {
          path: "config/app.json",
          content: '{"port": 8080, "debug": true, "tags": ["x", "y"]}',
        },
      },
    ],
  },
  {
    id: "write_unicode_content",
    tools: [WRITE_TOOL],
    prompt:
      'Append exactly this line to notes.txt: She said "héllo" — naïve café ✓',
    calls: [
      {
        tool: "write_file",
        args: {
          path: "notes.txt",
          content: 'She said "héllo" — naïve café ✓',
          append: true,
        },
      },
    ],
  },
  {
    id: "scale_integer_enum",
    tools: [SCALE_TOOL],
    prompt: "Scale pool api to 4 replicas as a dry run.",
    calls: [
      {
        tool: "scale_pool",
        args: { pool: "api", replicas: 4, dry_run: true },
      },
    ],
  },
  {
    id: "no_arguments",
    tools: [LIST_ALERTS_TOOL, BASH_TOOL],
    prompt: "List the active alerts.",
    calls: [{ tool: "list_active_alerts", args: {} }],
  },
];

function nullable(schema: JsonSchema, encoding: OptionalEncoding): JsonSchema {
  switch (encoding) {
    case OptionalEncoding.Omitted: {
      return schema;
    }
    case OptionalEncoding.AnyOfNull: {
      return { anyOf: [schema, { type: "null" }] };
    }
    case OptionalEncoding.TypeArrayNull: {
      const enumValues = schema["enum"];
      return {
        ...schema,
        type: [schema["type"], "null"],
        ...(Array.isArray(enumValues) ? { enum: [...enumValues, null] } : {}),
      };
    }
    default: {
      return encoding satisfies never;
    }
  }
}

function closeObjects(schema: JsonSchema): JsonSchema {
  const closed: Record<string, unknown> = { ...schema };
  const properties = schema["properties"];
  if (isRecord(properties)) {
    closed["additionalProperties"] = false;
    closed["properties"] = Object.fromEntries(
      Object.entries(properties).map(([key, child]) => [
        key,
        isRecord(child) ? closeObjects(child) : child,
      ])
    );
  }
  const items = schema["items"];
  if (isRecord(items)) {
    closed["items"] = closeObjects(items);
  }
  const anyOf = schema["anyOf"];
  if (Array.isArray(anyOf)) {
    closed["anyOf"] = anyOf.map((branch: unknown) =>
      isRecord(branch) ? closeObjects(branch) : branch
    );
  }
  return closed;
}

export function buildToolParameters(
  tool: ToolSpec,
  variant: SchemaVariant
): JsonSchema {
  const properties: Record<string, JsonSchema> = {};
  const required: string[] = [];
  for (const field of tool.fields) {
    if (field.optional === true) {
      properties[field.name] = {
        ...nullable(field.schema, variant.encoding),
        description: field.description,
      };
      if (variant.encoding !== OptionalEncoding.Omitted) {
        required.push(field.name);
      }
    } else {
      properties[field.name] = {
        ...field.schema,
        description: field.description,
      };
      required.push(field.name);
    }
  }
  const parameters: JsonSchema = { type: "object", properties, required };
  return variant.strict ? closeObjects(parameters) : parameters;
}

function toToolDefinition(
  tool: ToolSpec,
  variant: SchemaVariant
): ToolDefinition {
  return {
    type: "function",
    function: {
      name: tool.name,
      description: tool.description,
      parameters: buildToolParameters(tool, variant),
      ...(variant.strict ? { strict: true } : {}),
    },
  };
}

export function variantLabel(variant: SchemaVariant): string {
  return `${variant.encoding}${variant.strict ? "_strict" : ""}`;
}

export const TOOLCALL_FORMAT_CASES: readonly ToolCallFormatCase[] =
  SCENARIOS.flatMap((scenario) =>
    SCHEMA_VARIANTS.map((variant) => ({
      id: `toolcall_formats-${scenario.id}-${variantLabel(variant)}`,
      scenarioId: scenario.id,
      variant,
      prompt: scenario.prompt,
      tools: scenario.tools.map((tool) => toToolDefinition(tool, variant)),
      calls: scenario.calls,
    }))
  );

const CASES_BY_ID: ReadonlyMap<string, ToolCallFormatCase> = new Map(
  TOOLCALL_FORMAT_CASES.map((entry) => [entry.id, entry])
);

export function getToolCallFormatCase(
  id: string
): ToolCallFormatCase | undefined {
  return CASES_BY_ID.get(id);
}
