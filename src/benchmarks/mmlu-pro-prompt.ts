export interface MmluProCotExample {
  readonly question: string;
  readonly options: readonly string[];
  readonly cotContent: string;
}

export type MmluProCotExamplesByCategory = ReadonlyMap<
  string,
  readonly MmluProCotExample[]
>;

const MMLU_PRO_LETTERS = "ABCDEFGHIJ";

export function formatMmluProExample(
  question: string,
  options: readonly string[],
  cotContent: string
): string {
  let normalizedCotContent = cotContent;
  if (cotContent === "") {
    normalizedCotContent = "Let's think step by step.";
  } else if (cotContent.startsWith("A: ")) {
    normalizedCotContent = cotContent.slice(3);
  }
  const formattedOptions = options
    .filter((option) => option !== "N/A")
    .map((option, index) => `${MMLU_PRO_LETTERS[index]}. ${option}\n`)
    .join("");
  return `Question: ${question}\nOptions: ${formattedOptions}Answer: ${normalizedCotContent}\n\n`;
}

export function buildMmluProPrompt({
  category,
  cotExamples,
  question,
  options,
}: {
  readonly category: string;
  readonly cotExamples: readonly MmluProCotExample[];
  readonly question: string;
  readonly options: readonly string[];
}): string {
  const preamble = `The following are multiple choice questions (with answers) about ${category}. Think step by step and then output the answer in the format of "The answer is (X)" at the end.\n\n`;
  return (
    preamble +
    cotExamples
      .map((example) =>
        formatMmluProExample(
          example.question,
          example.options,
          example.cotContent
        )
      )
      .join("") +
    formatMmluProExample(question, options, "")
  );
}
