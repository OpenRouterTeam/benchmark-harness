import { seededPermutation } from "../scorers/mcq/shuffle";
import type {
  ChoiceQuestion,
  DecisionsQuestion,
  DecisionTaskId,
  DecisionTaskSpec,
} from "./schema";
import { DecisionVariant } from "./schema";
import type { BaseDecisionSpec } from "./tasks";

export const DISTRACTOR_OPTION = "unrelated_astronomy_query" as const;

export const DISTRACTOR_CRITERION =
  "The content is a question about stars, planets, or telescopes." as const;

export const EVIDENCE_REMOVED_STATE =
  "[The content for this item is unavailable.]" as const;

interface VariantInput {
  readonly spec: BaseDecisionSpec;
  readonly index: number;
  readonly task: DecisionTaskId;
  readonly questionId: string;
}

interface VariantOutput {
  readonly state: unknown;
  readonly stateText: string;
  readonly question: DecisionsQuestion;
  readonly labels: readonly string[];
  readonly gold: string;
  readonly goldKnowable: boolean;
}

function reorderChoice(
  question: ChoiceQuestion,
  order: readonly number[]
): ChoiceQuestion {
  const entries = Object.entries(question.criteria);
  return {
    ...question,
    criteria: Object.fromEntries(order.map((i) => entries[i]!)),
  };
}

function reorderLabels(
  labels: readonly string[],
  order: readonly number[]
): readonly string[] {
  return order.map((i) => labels[i]!);
}

function reordered(
  spec: BaseDecisionSpec,
  order: readonly number[]
): Pick<VariantOutput, "question" | "labels" | "gold"> {
  const { question } = spec;
  switch (question.type) {
    case "choice": {
      return {
        question: reorderChoice(question, order),
        labels: reorderLabels(spec.labels, order),
        gold: spec.gold,
      };
    }
    case "score": {
      const goldIndex = Number(spec.gold);
      return {
        question: {
          ...question,
          criteria: order.map((i) => question.criteria[i]!),
        },
        labels: spec.labels,
        gold: `${order.indexOf(goldIndex)}`,
      };
    }
    case "noul": {
      return { question, labels: spec.labels, gold: spec.gold };
    }
    default: {
      return question satisfies never;
    }
  }
}

function baseOutput(spec: BaseDecisionSpec): VariantOutput {
  return {
    state: spec.stateText,
    stateText: spec.stateText,
    question: spec.question,
    labels: spec.labels,
    gold: spec.gold,
    goldKnowable: true,
  };
}

function shuffledOutput(input: VariantInput): VariantOutput {
  const order = seededPermutation(input.spec.labels.length, input.index);
  return { ...baseOutput(input.spec), ...reordered(input.spec, order) };
}

function reversedOutput(input: VariantInput): VariantOutput {
  const order = input.spec.labels.map((_, i, all) => all.length - 1 - i);
  return { ...baseOutput(input.spec), ...reordered(input.spec, order) };
}

function distractorOutput(input: VariantInput): VariantOutput {
  const base = baseOutput(input.spec);
  if (input.spec.question.type !== "choice") {
    return base;
  }
  return {
    ...base,
    question: {
      ...input.spec.question,
      criteria: {
        ...input.spec.question.criteria,
        [DISTRACTOR_OPTION]: DISTRACTOR_CRITERION,
      },
    },
    labels: [...input.spec.labels, DISTRACTOR_OPTION],
  };
}

function noCriteriaOutput(input: VariantInput): VariantOutput {
  const base = baseOutput(input.spec);
  const { question } = input.spec;
  switch (question.type) {
    case "choice": {
      return {
        ...base,
        question: {
          ...question,
          criteria: Object.fromEntries(
            Object.keys(question.criteria).map((key) => [key, null])
          ),
        },
      };
    }
    case "noul": {
      const { criteria: _criteria, ...rest } = question;
      return { ...base, question: rest };
    }
    case "score": {
      return base;
    }
    default: {
      return question satisfies never;
    }
  }
}

function stateObjectOutput(input: VariantInput): VariantOutput {
  return {
    ...baseOutput(input.spec),
    state: {
      source: input.task,
      item_index: input.index,
      text: input.spec.stateText,
    },
  };
}

function evidenceRemovedOutput(input: VariantInput): VariantOutput {
  return {
    ...baseOutput(input.spec),
    state: EVIDENCE_REMOVED_STATE,
    stateText: EVIDENCE_REMOVED_STATE,
    goldKnowable: false,
  };
}

const VARIANT_BUILDERS: Readonly<
  Record<DecisionVariant, (input: VariantInput) => VariantOutput>
> = {
  [DecisionVariant.Base]: (input) => baseOutput(input.spec),
  [DecisionVariant.Shuffled]: shuffledOutput,
  [DecisionVariant.Reversed]: reversedOutput,
  [DecisionVariant.Distractor]: distractorOutput,
  [DecisionVariant.NoCriteria]: noCriteriaOutput,
  [DecisionVariant.StateObject]: stateObjectOutput,
  [DecisionVariant.EvidenceRemoved]: evidenceRemovedOutput,
};

export interface ApplyVariantInput extends VariantInput {
  readonly variant: DecisionVariant;
  readonly language: string;
}

export function applyVariant(input: ApplyVariantInput): DecisionTaskSpec {
  const output = VARIANT_BUILDERS[input.variant](input);
  return {
    task: input.task,
    variant: input.variant,
    language: input.language,
    questionId: input.questionId,
    state: output.state,
    stateText: output.stateText,
    question: output.question,
    labels: [...output.labels],
    gold: output.gold,
    goldKnowable: output.goldKnowable,
  };
}
