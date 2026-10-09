import { setTimeout as sleep } from "node:timers/promises";

import type { ToolCall } from "../../../harness/core";
import { Either } from "../../../internal/either";
import { parseSchema, z } from "../../../internal/zod";
import { TOOLCALL_SCHEMA_FUZZ_META } from "../../benchmark-meta";
import type { GridDraw } from "../grid";
import { scoreFuzzToolCalls } from "../scorer";
import { TOOLCALL_SCHEMA_FUZZ_SYSTEM_MESSAGE } from "../solver";
import { buildCase, FuzzScenario, requestOf } from "../tool-case";
import { rewriteProblems } from "./skeleton";
import type { WordingEntry } from "./wordings";
import { WordingEntrySchema } from "./wordings";

const RETRYABLE = new Set([408, 429, 500, 502, 503, 504]);
const HTTP_RETRIES = 6;

const WRITER_SYSTEM = `You turn synthetic tool-calling test cases into realistic ones. Each case tests how a serving stack handles one JSON Schema construct in a tool definition. You keep the construct and replace everything synthetic with something a real application and a real user would plausibly send.

Return one JSON object, nothing else:
{"name": string, "description": string, "parameters": object, "args": [object, object], "prompts": {"first": string, "second": string, "both": string}, "distractor": {"name": string, "description": string}}

Rules for "parameters":
- Keep the structure exactly: the same keywords, types, nesting, union branches in the same order, required and optional fields, additionalProperties, $ref/$defs layout, enum lengths, formats, and property order.
- You may rename property keys and $defs names (update $ref and required to match), add or change "description" on any schema, and change string or number literals in enum/const, patterns and numeric bounds, as long as both argument objects stay valid.
- Original keys are random identifiers; replace them with ordinary keys for the domain. Only where an original key contains spaces, punctuation other than "_" and "-", or non-ASCII characters, use realistic keys of that kind, e.g. header names, display labels or translated field names.

Rules for "args":
- Two argument objects, valid under your schema, with the same JSON type at every position as the originals: the same keys present in the same order, the same array lengths, integers stay integers, non-integer numbers stay non-integer, null stays null. If the originals differ, yours must differ.
- Original values are random draws; only their JSON types matter. Pick values a real user of this app would send, e.g. ordinary IDs, amounts, names and text. Where an original string contains unusual characters (control characters, quotes, backslashes, markup, escape sequences, non-Latin script, emoji), keep characters of that kind only where the domain naturally has them, e.g. pasted log lines, terminal input, file contents or user-generated text; never invent a contrived use just to keep them. Keep a number's magnitude only if it is plausible for the field.

Rules for "prompts":
- Each is a message from a real user of an app that has this tool. Do not dictate JSON or mention argument names mechanically; write a natural request.
- The request must determine the arguments exactly: every value is stated or unambiguously implied, text that must be passed verbatim is quoted or put in a code block as a user pasting it would, and omitted optional fields are not mentioned. A null value must be clearly requested as none/null/cleared.
- "first" asks for one call with args[0], "second" for one call with args[1], "both" asks in one message for two separate calls, one with each. For "both", make the two calls independent actions a user would naturally batch (e.g. two different items or two different accounts), never one call that overrides the other.

"name" and "distractor.name" are snake_case function names; the distractor is a different, plausible tool in the same app that this request must not use.`;

const ChatResponseSchema = z.object({
  choices: z.array(
    z.object({
      message: z.object({
        content: z.string().nullish(),
        tool_calls: z
          .array(
            z.object({
              id: z.string(),
              type: z.literal("function"),
              function: z.object({ name: z.string(), arguments: z.string() }),
            })
          )
          .nullish(),
      }),
    })
  ),
});
type ChatMessage = z.infer<
  typeof ChatResponseSchema
>["choices"][number]["message"];
export type Complete = (
  body: Readonly<Record<string, unknown>>
) => Promise<ChatMessage | string>;

export interface Rewriter {
  readonly complete: Complete;
  readonly writer: string;
  readonly checker: string;
  readonly attempts: number;
  readonly log: (line: string) => void;
}

const DraftSchema = WordingEntrySchema.omit({ key: true });
type Draft = z.infer<typeof DraftSchema>;

export function openRouterChat({
  apiKey,
  baseUrl,
}: {
  readonly apiKey: string;
  readonly baseUrl: string;
}): Complete {
  return async (body) => {
    for (let attempt = 0; attempt <= HTTP_RETRIES; attempt += 1) {
      const response = await fetch(`${baseUrl}/chat/completions`, {
        method: "POST",
        headers: {
          authorization: `Bearer ${apiKey}`,
          "content-type": "application/json",
        },
        body: JSON.stringify(body),
      });
      const text = await response.text();
      if (RETRYABLE.has(response.status)) {
        await sleep(2 ** attempt * 1000);
        continue;
      }
      const json = Either.try((): unknown => JSON.parse(text));
      const parsed = Either.isLeft(json)
        ? json
        : parseSchema(ChatResponseSchema, json.right);
      const message = Either.isLeft(parsed)
        ? undefined
        : parsed.right.choices[0]?.message;
      return message ?? `HTTP ${response.status}`;
    }
    return "retries exhausted";
  };
}

function caseBrief(key: string, drawn: GridDraw): string {
  return JSON.stringify(
    {
      construct: key,
      original: { parameters: drawn.parameters, args: drawn.args },
      argsDiffer: drawn.isDistinct,
    },
    null,
    2
  );
}

function parseDraft(content: string): Draft | string {
  const json = Either.try((): unknown =>
    JSON.parse(
      content.slice(content.indexOf("{"), content.lastIndexOf("}") + 1)
    )
  );
  if (Either.isLeft(json)) {
    return "reply is not valid JSON";
  }
  const parsed = parseSchema(DraftSchema, json.right);
  return Either.isLeft(parsed)
    ? `not a JSON object of the requested form: ${parsed.left.message}`
    : parsed.right;
}

function checks(
  key: string,
  draft: Draft
): readonly ReturnType<typeof buildCase>[] {
  const base = {
    id: key,
    index: 0,
    strict: false,
    name: draft.name,
    parameters: draft.parameters,
    constructs: [],
    wording: {
      description: draft.description,
      prompts: draft.prompts,
      distractor: draft.distractor,
    },
  };
  const isDistinct =
    JSON.stringify(draft.args[0]) !== JSON.stringify(draft.args[1]);
  const swapped = {
    ...base,
    args: [draft.args[1], draft.args[0]] as const,
    wording: {
      ...base.wording,
      prompts: { ...draft.prompts, first: draft.prompts.second },
    },
  };
  return [
    buildCase({ ...base, scenario: FuzzScenario.Distractor, args: draft.args }),
    buildCase({ ...swapped, scenario: FuzzScenario.Single }),
    ...(isDistinct && draft.prompts.both !== undefined
      ? [
          buildCase({
            ...base,
            scenario: FuzzScenario.Parallel,
            args: draft.args,
          }),
        ]
      : []),
  ];
}

interface CheckerProblems {
  readonly single: readonly string[];
  readonly parallel: readonly string[];
}

function promptName(scenario: FuzzScenario): string {
  switch (scenario) {
    case FuzzScenario.Parallel: {
      return '"both"';
    }
    case FuzzScenario.Single: {
      return '"second"';
    }
    case FuzzScenario.Replay:
    case FuzzScenario.Distractor: {
      return '"first"';
    }
    default: {
      return scenario satisfies never;
    }
  }
}

async function checkerProblems(
  rewriter: Rewriter,
  { key, draft }: { readonly key: string; readonly draft: Draft }
): Promise<CheckerProblems> {
  const results = await Promise.all(
    checks(key, draft).map(async (entry) => {
      const message = await rewriter.complete({
        ...requestOf(entry, TOOLCALL_SCHEMA_FUZZ_SYSTEM_MESSAGE),
        model: rewriter.checker,
        temperature: TOOLCALL_SCHEMA_FUZZ_META.temperature,
      });
      if (typeof message === "string") {
        return { entry, problem: `checker request failed (${message})` };
      }
      const toolCalls: readonly ToolCall[] = message.tool_calls ?? [];
      const score = scoreFuzzToolCalls(entry, toolCalls);
      const made = JSON.stringify(
        toolCalls.map((call) => call.function.arguments)
      );
      return {
        entry,
        problem:
          score.value === "C"
            ? undefined
            : `for the ${promptName(entry.scenario)} prompt another model made the calls ${made} (${score.explanation}); make the request state the arguments unambiguously`,
      };
    })
  );
  const problemsOf = (isParallel: boolean): readonly string[] =>
    results.flatMap(({ entry, problem }) =>
      problem !== undefined &&
      (entry.scenario === FuzzScenario.Parallel) === isParallel
        ? [problem]
        : []
    );
  return { single: problemsOf(false), parallel: problemsOf(true) };
}

function withoutBoth(key: string, draft: Draft): WordingEntry {
  const { both: _both, ...prompts } = draft.prompts;
  return { key, ...draft, prompts };
}

async function draftOutcome(
  rewriter: Rewriter,
  {
    key,
    drawn,
    draft,
    attempt,
  }: {
    readonly key: string;
    readonly drawn: GridDraw;
    readonly draft: Draft;
    readonly attempt: number;
  }
): Promise<
  { readonly entry: WordingEntry } | { readonly problems: readonly string[] }
> {
  const structural = rewriteProblems({
    original: drawn,
    rewrite: draft,
    distractorName: draft.distractor.name,
  });
  if (structural.length > 0) {
    return { problems: structural };
  }
  const { single, parallel } = await checkerProblems(rewriter, { key, draft });
  if (single.length === 0 && parallel.length === 0) {
    return { entry: { key, ...draft } };
  }
  if (single.length === 0 && attempt === rewriter.attempts - 1) {
    rewriter.log(
      `kept ${key} without a parallel request: ${parallel.join("; ")}`
    );
    return { entry: withoutBoth(key, draft) };
  }
  return { problems: [...single, ...parallel] };
}

export async function reword(
  rewriter: Rewriter,
  { key, drawn }: { readonly key: string; readonly drawn: GridDraw }
): Promise<WordingEntry | string> {
  const messages: { role: "system" | "user" | "assistant"; content: string }[] =
    [
      { role: "system", content: WRITER_SYSTEM },
      { role: "user", content: caseBrief(key, drawn) },
    ];
  let last = "no attempts";
  for (let attempt = 0; attempt < rewriter.attempts; attempt += 1) {
    const reply = await rewriter.complete({
      model: rewriter.writer,
      messages,
      temperature: 0.7,
    });
    const content = typeof reply === "string" ? undefined : reply.content;
    if (content === undefined || content === null) {
      last = typeof reply === "string" ? reply : "empty writer reply";
      continue;
    }
    const draft = parseDraft(content);
    const outcome =
      typeof draft === "string"
        ? { problems: [draft] }
        : await draftOutcome(rewriter, { key, drawn, draft, attempt });
    if ("entry" in outcome) {
      return outcome.entry;
    }
    const { problems } = outcome;
    last = problems.join("; ");
    rewriter.log(`retry ${key} #${attempt}: ${last}`);
    messages.push(
      { role: "assistant", content },
      {
        role: "user",
        content: `Fix these problems and return the full JSON object again:\n- ${problems.join("\n- ")}`,
      }
    );
  }
  return last;
}
