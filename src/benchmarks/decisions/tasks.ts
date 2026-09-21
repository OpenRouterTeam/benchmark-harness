import { Either } from "../../internal/either";
import { isMember } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import {
  BANKING77_LABELS,
  DBPEDIA_14_LABELS,
  MASSIVE_INTENT_LABELS,
  NOUL_LABELS,
  SST5_LEVELS,
  XNLI_LABELS,
} from "./label-spaces";
import type { DecisionsQuestion, DecisionTaskId } from "./schema";
import { DecisionTaskId as TaskId } from "./schema";

export const DEFAULT_LANGUAGE = "en" as const;

export const MMLU_PRO_OPTION_LETTERS = "ABCDEFGHIJ" as const;

export interface DecisionTaskSource {
  readonly dataset: string;
  readonly config: string;
  readonly split: string;
}

export interface BaseDecisionSpec {
  readonly stateText: string;
  readonly question: DecisionsQuestion;
  readonly labels: readonly string[];
  readonly gold: string;
}

export interface DecisionTaskDefinition {
  readonly id: DecisionTaskId;
  readonly questionId: string;
  readonly license: string;
  readonly source: (language: string) => DecisionTaskSource;
  readonly recordToSpec: (
    record: Readonly<Record<string, unknown>>
  ) => BaseDecisionSpec;
}

function invalidRecord(task: DecisionTaskId, message: string): never {
  throw new TypeError(`${task} record failed validation: ${message}`);
}

function parseRecord<Output>(
  task: DecisionTaskId,
  schema: z.ZodType<Output, unknown>,
  record: Readonly<Record<string, unknown>>
): Output {
  const parsed = parseSchema(schema, record);
  return Either.isLeft(parsed)
    ? invalidRecord(task, parsed.left.message)
    : parsed.right;
}

export function humanizeLabel(label: string): string {
  return label
    .replaceAll("_", " ")
    .replaceAll(/([a-z])([A-Z])/gu, "$1 $2")
    .toLowerCase();
}

function labelCriteria(
  labels: readonly string[],
  describe: (label: string) => string
): Record<string, string> {
  return Object.fromEntries(labels.map((label) => [label, describe(label)]));
}

const LabelTextRecordSchema = z.object({
  text: z.string(),
  label_text: z.string(),
});

const Banking77Task: DecisionTaskDefinition = {
  id: TaskId.Banking77,
  questionId: "intent",
  license: "MIT (mteb/banking77 card), original PolyAI/banking77 CC-BY-4.0",
  source: () => ({
    dataset: "mteb/banking77",
    config: "default",
    split: "test",
  }),
  recordToSpec: (record) => {
    const row = parseRecord(TaskId.Banking77, LabelTextRecordSchema, record);
    if (!isMember(row.label_text, BANKING77_LABELS)) {
      invalidRecord(TaskId.Banking77, `unknown label ${row.label_text}`);
    }
    return {
      stateText: row.text,
      question: {
        type: "choice",
        instructions:
          "Which banking intent does the customer message express? Pick the single best matching intent.",
        criteria: labelCriteria(
          BANKING77_LABELS,
          (label) => `The customer's message is about: ${humanizeLabel(label)}.`
        ),
      },
      labels: BANKING77_LABELS,
      gold: row.label_text,
    };
  },
};

const MassiveRecordSchema = z.object({
  text: z.string(),
  label: z.string(),
});

const MassiveIntentTask: DecisionTaskDefinition = {
  id: TaskId.MassiveIntent,
  questionId: "intent",
  license:
    "Apache-2.0 (mteb/amazon_massive_intent card), original MASSIVE CC-BY-4.0",
  source: (language) => ({
    dataset: "mteb/amazon_massive_intent",
    config: language,
    split: "test",
  }),
  recordToSpec: (record) => {
    const row = parseRecord(TaskId.MassiveIntent, MassiveRecordSchema, record);
    if (!isMember(row.label, MASSIVE_INTENT_LABELS)) {
      invalidRecord(TaskId.MassiveIntent, `unknown label ${row.label}`);
    }
    return {
      stateText: row.text,
      question: {
        type: "choice",
        instructions:
          "Which voice-assistant intent does the utterance express? Pick the single best matching intent.",
        criteria: labelCriteria(
          MASSIVE_INTENT_LABELS,
          (label) => `The user wants: ${humanizeLabel(label)}.`
        ),
      },
      labels: MASSIVE_INTENT_LABELS,
      gold: row.label,
    };
  },
};

const DbpediaRecordSchema = z.object({
  title: z.string(),
  content: z.string(),
  label: z
    .number()
    .int()
    .min(0)
    .max(DBPEDIA_14_LABELS.length - 1),
});

const Dbpedia14Task: DecisionTaskDefinition = {
  id: TaskId.Dbpedia14,
  questionId: "category",
  license: "CC-BY-SA-3.0 (fancyzhx/dbpedia_14 card)",
  source: () => ({
    dataset: "fancyzhx/dbpedia_14",
    config: "dbpedia_14",
    split: "test",
  }),
  recordToSpec: (record) => {
    const row = parseRecord(TaskId.Dbpedia14, DbpediaRecordSchema, record);
    return {
      stateText: `${row.title}\n\n${row.content.trim()}`,
      question: {
        type: "choice",
        instructions:
          "Which DBpedia ontology class does the article subject belong to?",
        criteria: labelCriteria(
          DBPEDIA_14_LABELS,
          (label) => `The subject is a ${humanizeLabel(label)}.`
        ),
      },
      labels: DBPEDIA_14_LABELS,
      gold: DBPEDIA_14_LABELS[row.label]!,
    };
  },
};

const BoolQRecordSchema = z.object({
  question: z.string(),
  passage: z.string(),
  answer: z.boolean(),
});

const BoolQTask: DecisionTaskDefinition = {
  id: TaskId.BoolQ,
  questionId: "answer_is_yes",
  license: "CC-BY-SA-3.0 (google/boolq card)",
  source: () => ({
    dataset: "google/boolq",
    config: "default",
    split: "validation",
  }),
  recordToSpec: (record) => {
    const row = parseRecord(TaskId.BoolQ, BoolQRecordSchema, record);
    return {
      stateText: `Passage: ${row.passage}\n\nQuestion: ${row.question}?`,
      question: {
        type: "noul",
        instructions:
          "According to the passage, is the answer to the question yes?",
        criteria: {
          true: "The passage supports answering the question with yes.",
          false: "The passage supports answering the question with no.",
        },
      },
      labels: NOUL_LABELS,
      gold: row.answer ? "true" : "false",
    };
  },
};

const PromptInjectionRecordSchema = z.object({
  text: z.string(),
  label: z.union([z.literal(0), z.literal(1)]),
});

const PromptInjectionTask: DecisionTaskDefinition = {
  id: TaskId.PromptInjection,
  questionId: "is_injection",
  license: "Apache-2.0 (deepset/prompt-injections card)",
  source: () => ({
    dataset: "deepset/prompt-injections",
    config: "default",
    split: "test",
  }),
  recordToSpec: (record) => {
    const row = parseRecord(
      TaskId.PromptInjection,
      PromptInjectionRecordSchema,
      record
    );
    return {
      stateText: row.text,
      question: {
        type: "noul",
        instructions:
          "Is this user input a prompt injection that tries to override, hijack, or manipulate the assistant's instructions?",
        criteria: {
          true: "The input attempts to override or manipulate the assistant's instructions or role.",
          false:
            "The input is an ordinary request or statement with no attempt to manipulate the assistant.",
        },
      },
      labels: NOUL_LABELS,
      gold: row.label === 1 ? "true" : "false",
    };
  },
};

const Sst5RecordSchema = z.object({
  text: z.string(),
  label: z
    .number()
    .int()
    .min(0)
    .max(SST5_LEVELS.length - 1),
});

const SST5_LEVEL_KEYS = SST5_LEVELS.map((_, index) => `${index}`);

const Sst5Task: DecisionTaskDefinition = {
  id: TaskId.Sst5,
  questionId: "sentiment",
  license:
    "Not declared on SetFit/sst5 card (Stanford Sentiment Treebank); verify before publication",
  source: () => ({
    dataset: "SetFit/sst5",
    config: "default",
    split: "test",
  }),
  recordToSpec: (record) => {
    const row = parseRecord(TaskId.Sst5, Sst5RecordSchema, record);
    return {
      stateText: row.text,
      question: {
        type: "score",
        instructions:
          "How positive is the sentiment of this movie review sentence?",
        criteria: SST5_LEVELS.map((level) => `The sentiment is ${level}.`),
      },
      labels: SST5_LEVEL_KEYS,
      gold: `${row.label}`,
    };
  },
};

const MmluProRecordSchema = z.object({
  question: z.string(),
  options: z.array(z.string()).min(2).max(MMLU_PRO_OPTION_LETTERS.length),
  answer: z.string().length(1),
  answer_index: z.number().int().min(0),
});

const MmluProTask: DecisionTaskDefinition = {
  id: TaskId.MmluPro,
  questionId: "answer",
  license: "MIT (TIGER-Lab/MMLU-Pro card)",
  source: () => ({
    dataset: "TIGER-Lab/MMLU-Pro",
    config: "default",
    split: "test",
  }),
  recordToSpec: (record) => {
    const row = parseRecord(TaskId.MmluPro, MmluProRecordSchema, record);
    const options = row.options.filter((option) => option !== "N/A");
    const letters = options.map((_, index) => MMLU_PRO_OPTION_LETTERS[index]!);
    if (
      row.answer_index >= options.length ||
      row.answer !== letters[row.answer_index]
    ) {
      invalidRecord(
        TaskId.MmluPro,
        `answer ${row.answer} does not index options`
      );
    }
    return {
      stateText: row.question,
      question: {
        type: "choice",
        instructions: "Which option is the correct answer to the question?",
        criteria: Object.fromEntries(
          letters.map((letter, index) => [letter, options[index]!])
        ),
      },
      labels: letters,
      gold: row.answer,
    };
  },
};

const XnliRecordSchema = z.object({
  premise: z.string(),
  hypothesis: z.string(),
  label: z
    .number()
    .int()
    .min(0)
    .max(XNLI_LABELS.length - 1),
});

const XnliTask: DecisionTaskDefinition = {
  id: TaskId.Xnli,
  questionId: "relation",
  license:
    "Not declared on facebook/xnli card; XNLI is distributed under CC-BY-NC-4.0, non-commercial",
  source: (language) => ({
    dataset: "facebook/xnli",
    config: language,
    split: "test",
  }),
  recordToSpec: (record) => {
    const row = parseRecord(TaskId.Xnli, XnliRecordSchema, record);
    return {
      stateText: `Premise: ${row.premise}\n\nHypothesis: ${row.hypothesis}`,
      question: {
        type: "choice",
        instructions:
          "What is the logical relation between the premise and the hypothesis?",
        criteria: {
          entailment: "The hypothesis must be true if the premise is true.",
          neutral: "The hypothesis may or may not be true given the premise.",
          contradiction:
            "The hypothesis cannot be true if the premise is true.",
        },
      },
      labels: XNLI_LABELS,
      gold: XNLI_LABELS[row.label]!,
    };
  },
};

export const DECISION_TASKS: Readonly<
  Record<DecisionTaskId, DecisionTaskDefinition>
> = {
  [TaskId.Banking77]: Banking77Task,
  [TaskId.MassiveIntent]: MassiveIntentTask,
  [TaskId.Dbpedia14]: Dbpedia14Task,
  [TaskId.BoolQ]: BoolQTask,
  [TaskId.PromptInjection]: PromptInjectionTask,
  [TaskId.Sst5]: Sst5Task,
  [TaskId.MmluPro]: MmluProTask,
  [TaskId.Xnli]: XnliTask,
};

export function decisionTask(id: DecisionTaskId): DecisionTaskDefinition {
  return DECISION_TASKS[id];
}
