import type { DecisionsQuestion, DecisionTaskSpec } from "./schema";

export const LLM_RESPONSE_SHAPE =
  '{"answer": "<option label>", "confidence": <number between 0 and 1>}' as const;

export const LLM_SYSTEM_PROMPT = [
  "You are a typed decision function.",
  "You receive a STATE, a QUESTION, and a closed set of OPTIONS.",
  "Choose exactly one option label from OPTIONS that best answers the QUESTION about the STATE.",
  `Respond with a single JSON object of the form ${LLM_RESPONSE_SHAPE} and nothing else.`,
  "Copy the option label exactly as written. Do not invent labels, do not explain.",
  "confidence is your probability that the chosen option is the correct one.",
].join(" ");

function renderText(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

function renderOptions(
  question: DecisionsQuestion,
  labels: readonly string[]
): string {
  switch (question.type) {
    case "noul": {
      const criteria = question.criteria;
      if (criteria === undefined) {
        return labels.map((label) => `- ${label}`).join("\n");
      }
      return labels
        .map((label) => {
          const detail = renderText(
            label === "true" ? criteria.true : criteria.false
          );
          return `- ${label}: ${detail}`;
        })
        .join("\n");
    }
    case "choice": {
      return labels
        .map((label) => {
          const detail = question.criteria[label];
          return detail === null || detail === undefined
            ? `- ${label}`
            : `- ${label}: ${renderText(detail)}`;
        })
        .join("\n");
    }
    case "score": {
      return labels
        .map((label) => {
          const detail = question.criteria[Number(label)];
          return detail === undefined
            ? `- ${label}`
            : `- ${label}: ${renderText(detail)}`;
        })
        .join("\n");
    }
    default: {
      return question satisfies never;
    }
  }
}

export function renderLlmPrompt(spec: DecisionTaskSpec): string {
  return [
    "STATE:",
    renderText(spec.state),
    "",
    "QUESTION:",
    renderText(spec.question.instructions),
    "",
    "OPTIONS:",
    renderOptions(spec.question, spec.labels),
    "",
    `Respond with ${LLM_RESPONSE_SHAPE}.`,
  ].join("\n");
}
