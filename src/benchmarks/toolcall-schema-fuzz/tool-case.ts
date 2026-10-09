import type { Arbitrary } from "fast-check";
import { stringMatching } from "fast-check";

import type { ValueOf } from "../../internal/guards";
import type { JsonSchema } from "./json-schema";

export const FuzzConstruct = {
  String: "string",
  Integer: "integer",
  Number: "number",
  Boolean: "boolean",
  Null: "null",
  Enum: "enum",
  Const: "const",
  AnySchema: "any_schema",
  TypeArray: "type_array",
  Nullable: "nullable",
  Object: "object",
  OpenObject: "open_object",
  OptionalProperty: "optional_property",
  Array: "array",
  AnyOf: "anyOf",
  OneOf: "oneOf",
  AllOf: "allOf",
  Ref: "$ref",
  RecursiveRef: "recursive_ref",
  StringBounds: "string_bounds",
  Pattern: "pattern",
  Format: "format",
  NumberBounds: "number_bounds",
  MultipleOf: "multipleOf",
  ArrayBounds: "array_bounds",
  PrefixItems: "prefixItems",
  AdditionalPropertiesSchema: "additionalProperties_schema",
  Description: "description",
  UnusualKey: "unusual_key",
  Not: "not",
  Conditional: "if_then_else",
  Contains: "contains",
  UniqueItems: "uniqueItems",
  PropertyBounds: "property_bounds",
  PatternProperties: "patternProperties",
  PropertyNames: "propertyNames",
  DependentRequired: "dependentRequired",
  DependentSchemas: "dependentSchemas",
  Annotation: "annotation",
  ContentAnnotation: "content_annotation",
  Definitions: "definitions",
  RefSiblings: "ref_siblings",
  EscapedPointer: "escaped_pointer",
  SingleAllOf: "single_allOf",
  RootCombinator: "root_combinator",
  CompositeTypeArray: "composite_type_array",
  BooleanSchema: "boolean_schema",
  SingleBranchUnion: "single_branch_union",
  WideCombinator: "wide_combinator",
} as const;
export type FuzzConstruct = ValueOf<typeof FuzzConstruct>;

export const FuzzScenario = {
  Single: "single",

  Replay: "replay",

  Distractor: "distractor",

  Parallel: "parallel",
} as const;
export type FuzzScenario = ValueOf<typeof FuzzScenario>;

export interface ToolDefinition {
  readonly type: "function";
  readonly function: {
    readonly name: string;
    readonly description: string;
    readonly parameters: JsonSchema;
    readonly strict: boolean;
  };
}

export type FuzzMessage =
  | { readonly role: "user"; readonly content: string }
  | {
      readonly role: "assistant";
      readonly content: string;
      readonly tool_calls: readonly {
        readonly id: string;
        readonly type: "function";
        readonly function: {
          readonly name: string;
          readonly arguments: string;
        };
      }[];
    }
  | {
      readonly role: "tool";
      readonly tool_call_id: string;
      readonly content: string;
    };

export interface ExpectedCall {
  readonly name: string;
  readonly args: Readonly<Record<string, unknown>>;
}

export interface ToolCallSchemaFuzzCase {
  readonly id: string;
  readonly index: number;
  readonly scenario: FuzzScenario;

  readonly prompt: string;

  readonly messages: readonly FuzzMessage[];
  readonly tools: readonly ToolDefinition[];

  readonly calls: readonly ExpectedCall[];
  readonly strict: boolean;
  readonly constructs: readonly FuzzConstruct[];

  readonly control?: string;
}

export const toolNames: Arbitrary<string> = stringMatching(
  /^[A-Za-z][\w-]{0,31}$/
);

const COPY_RULE =
  "Copy every value verbatim: keep each value's JSON type, every character of every string (whitespace, quotes, backslashes, markup), and do not add or drop keys.";

function jsonBlock(args: unknown): readonly string[] {
  return ["```json", JSON.stringify(args, null, 2), "```"];
}

function singlePrompt(name: string, args: unknown): string {
  return [
    `Call the \`${name}\` tool exactly once with exactly these arguments.`,
    COPY_RULE,
    "",
    ...jsonBlock(args),
  ].join("\n");
}

function parallelPrompt(
  name: string,
  [first, second]: readonly unknown[]
): string {
  return [
    `Call the \`${name}\` tool exactly twice, as two parallel tool calls in this one response: once with the first arguments and once with the second.`,
    COPY_RULE,
    "",
    "First call:",
    ...jsonBlock(first),
    "",
    "Second call:",
    ...jsonBlock(second),
  ].join("\n");
}

export interface CaseInput {
  readonly id: string;
  readonly index: number;
  readonly scenario: FuzzScenario;
  readonly name: string;
  readonly strict: boolean;
  readonly parameters: JsonSchema;

  readonly args: readonly [
    Readonly<Record<string, unknown>>,
    Readonly<Record<string, unknown>>,
  ];
  readonly constructs: readonly FuzzConstruct[];
  readonly control?: string;

  readonly wording?: CaseWording;
}

export interface CaseWording {
  readonly description: string;

  readonly prompts: {
    readonly first: string;
    readonly second: string;
    readonly both?: string;
  };
  readonly distractor: { readonly name: string; readonly description: string };
}

function toolOf({
  name,
  parameters,
  strict,
  description = `Fuzz tool ${name}. Pass the arguments exactly as given.`,
}: Pick<CaseInput, "name" | "parameters" | "strict"> & {
  readonly description?: string;
}): ToolDefinition {
  return {
    type: "function",
    function: { name, description, parameters, strict },
  };
}

function promptsOf(input: CaseInput): Required<CaseWording["prompts"]> {
  const synthetic = {
    first: singlePrompt(input.name, input.args[0]),
    second: singlePrompt(input.name, input.args[1]),
    both: parallelPrompt(input.name, input.args),
  };
  return { ...synthetic, ...input.wording?.prompts };
}

function targetToolOf(input: CaseInput): ToolDefinition {
  return toolOf({
    ...input,
    ...(input.wording === undefined
      ? {}
      : { description: input.wording.description }),
  });
}

function distractorToolOf(input: CaseInput): ToolDefinition {
  return input.wording === undefined
    ? toolOf({ ...input, name: `${input.name}_v2` })
    : toolOf({ ...input, ...input.wording.distractor });
}

type Conversation = Pick<
  ToolCallSchemaFuzzCase,
  "messages" | "tools" | "calls"
>;

const SCENARIOS: Readonly<
  Record<FuzzScenario, (input: CaseInput) => Conversation>
> = {
  single: (input) => ({
    messages: [{ role: "user", content: promptsOf(input).first }],
    tools: [targetToolOf(input)],
    calls: [{ name: input.name, args: input.args[0] }],
  }),
  replay: (input) => ({
    messages: [
      { role: "user", content: promptsOf(input).second },
      {
        role: "assistant",
        content: "",
        tool_calls: [
          {
            id: "call_0",
            type: "function",
            function: {
              name: input.name,
              arguments: JSON.stringify(input.args[1]),
            },
          },
        ],
      },
      { role: "tool", tool_call_id: "call_0", content: '{"ok":true}' },
      { role: "user", content: promptsOf(input).first },
    ],
    tools: [targetToolOf(input)],
    calls: [{ name: input.name, args: input.args[0] }],
  }),
  distractor: (input) => ({
    messages: [{ role: "user", content: promptsOf(input).first }],
    tools: [distractorToolOf(input), targetToolOf(input)],
    calls: [{ name: input.name, args: input.args[0] }],
  }),
  parallel: (input) => ({
    messages: [{ role: "user", content: promptsOf(input).both }],
    tools: [targetToolOf(input)],
    calls: input.args.map((args) => ({ name: input.name, args })),
  }),
};

function repeatPrompt(conversation: Conversation): string {
  const [first] = conversation.calls;
  const name = first?.name ?? "";
  const ask =
    conversation.calls.length > 1
      ? `Make the same ${conversation.calls.length} \`${name}\` calls again, as parallel tool calls in this one response, with exactly the same arguments as your last turn.`
      : `Call the \`${name}\` tool again with exactly the same arguments as your last call.`;
  return [ask, COPY_RULE].join("\n");
}

function priorCallCount(messages: readonly FuzzMessage[]): number {
  return messages.reduce(
    (count, message) =>
      count + (message.role === "assistant" ? message.tool_calls.length : 0),
    0
  );
}

function withCopyTurn(conversation: Conversation): Conversation {
  const offset = priorCallCount(conversation.messages);
  const ids = conversation.calls.map((_, index) => `call_${offset + index}`);
  return {
    ...conversation,
    messages: [
      ...conversation.messages,
      {
        role: "assistant",
        content: "",
        tool_calls: conversation.calls.map((call, index) => ({
          id: ids[index] ?? "",
          type: "function",
          function: { name: call.name, arguments: JSON.stringify(call.args) },
        })),
      },
      ...ids.map((id) => ({
        role: "tool" as const,
        tool_call_id: id,
        content: '{"ok":true}',
      })),
      { role: "user", content: repeatPrompt(conversation) },
    ],
  };
}

function caseOf(
  input: CaseInput,
  conversation: Conversation
): ToolCallSchemaFuzzCase {
  const last = conversation.messages.at(-1);
  return {
    id: input.id,
    index: input.index,
    scenario: input.scenario,
    prompt: last?.role === "user" ? last.content : "",
    ...conversation,
    strict: input.strict,
    constructs: input.constructs,
    ...(input.control === undefined ? {} : { control: input.control }),
  };
}

export function buildCase(input: CaseInput): ToolCallSchemaFuzzCase {
  return caseOf(input, withCopyTurn(SCENARIOS[input.scenario](input)));
}

export function requestOf(
  entry: ToolCallSchemaFuzzCase,
  systemMessage: string
): Readonly<Record<string, unknown>> {
  return {
    messages: [{ role: "system", content: systemMessage }, ...entry.messages],
    tools: entry.tools,
    ...(entry.scenario === FuzzScenario.Parallel
      ? { parallel_tool_calls: true }
      : {}),
  };
}

export function roundTripJson(value: unknown): unknown {
  return JSON.parse(JSON.stringify(value));
}
