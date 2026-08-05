export function buildDynamicMcqPrompt(
  question: string,
  options: readonly string[]
): string {
  const letterLabels = options.map(
    (option, index) => `${String.fromCodePoint(65 + index)}) ${option}`
  );
  const letters = options.map((_, index) => String.fromCodePoint(65 + index));
  return `Answer the following multiple choice question. The last line of your response should be of the following format: 'Answer: $LETTER' (without quotes) where LETTER is one of ${letters.join("")}.\n\n${question}\n\n${letterLabels.join("\n")}`;
}
