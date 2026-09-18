import type { Effect } from "effect/Effect";
import { fail, gen, succeed } from "effect/Effect";

import type { ModelError, ModelMessage, ModelUsage } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import { withCallCacheSalt } from "../../runtime/response-cache";
import type { Value } from "./probably/language";

const LETTERS = "ABCDEFGH";

export const JUDGE_SYSTEM_PROMPT = [
  "You are the judgment step of a Trust and Safety decision program.",
  "You receive a case dossier and a set of labelled descriptions.",
  "Estimate the probability that each description is the correct characterisation of the case.",
  "Consider only the evidence in the dossier. Do not assume facts that are not stated.",
  "Respond with a single JSON object mapping each letter to a probability. The probabilities must sum to 1.",
  'Example: {"A": 0.7, "B": 0.3}',
].join("\n");

export interface JudgeCall {
  readonly probabilities: Readonly<Record<string, number>>;
  readonly usage: ModelUsage | undefined;
  readonly generationTimeMs: number;
  readonly completion: string;
}

export function judgePrompt(value: Value, labels: readonly string[]): string {
  const options = labels
    .map((label, i) => `${LETTERS[i] ?? String(i)}. ${label}`)
    .join("\n");
  return `Dossier:\n${String(value)}\n\nDescriptions:\n${options}\n\nReturn the JSON probabilities.`;
}

function extractJson(text: string): unknown {
  const direct = Either.try((): unknown => JSON.parse(text.trim()));
  if (Either.isRight(direct)) {
    return direct.right;
  }
  const start = text.indexOf("{");
  const end = text.lastIndexOf("}");
  if (start === -1 || end <= start) {
    return undefined;
  }
  const slice = Either.try((): unknown =>
    JSON.parse(text.slice(start, end + 1))
  );
  return Either.isRight(slice) ? slice.right : undefined;
}

export function parseJudgeCompletion(
  completion: string,
  labels: readonly string[]
): Record<string, number> | undefined {
  const json = extractJson(completion);
  const body =
    isRecord(json) && isRecord(json["probabilities"])
      ? json["probabilities"]
      : json;
  if (!isRecord(body)) {
    return undefined;
  }
  const entries = labels.map(
    (label, i): readonly [string, number] | undefined => {
      const letter = LETTERS[i] ?? String(i);
      const raw = body[letter] ?? body[label];
      const n = typeof raw === "string" ? Number(raw) : raw;
      return typeof n === "number" && Number.isFinite(n) && n >= 0
        ? [label, n]
        : undefined;
    }
  );
  if (entries.some((e) => e === undefined)) {
    return undefined;
  }
  const values = entries.flatMap((e) => (e === undefined ? [] : [e]));
  const sum = values.reduce((acc, [, n]) => acc + n, 0);
  if (sum <= 0) {
    return undefined;
  }
  return Object.fromEntries(values.map(([label, n]) => [label, n / sum]));
}

export function judgeWithChatModel(
  model: ModelService,
  config: GenerateConfig,
  callSalt: string,
  value: Value,
  labels: readonly string[]
): Effect<JudgeCall, ModelError | SolverError> {
  return gen(function* () {
    const messages: readonly ModelMessage[] = [
      { role: MessageRole.System, content: JUDGE_SYSTEM_PROMPT },
      { role: MessageRole.User, content: judgePrompt(value, labels) },
    ];
    const output = yield* withCallCacheSalt(
      callSalt,
      model.generate(messages, config)
    );
    const probabilities = parseJudgeCompletion(output.completion, labels);
    if (probabilities === undefined) {
      return yield* fail(
        new SolverError({
          message: `Judge returned an unparseable distribution for ${labels.length} labels`,
        })
      );
    }
    return yield* succeed({
      probabilities,
      usage: output.usage,
      generationTimeMs: output.generationTimeMs ?? 0,
      completion: output.completion,
    });
  });
}
