export function mmluProExtractAnswer(text: string): string | null {
  const cleaned = text.replaceAll("**", "");
  const answerIsMatch = cleaned.match(/answer is \(?([A-J])\)?/);
  if (answerIsMatch?.[1] !== undefined) {
    return answerIsMatch[1];
  }
  const answerColonMatch = cleaned.match(/.*[aA]nswer:\s*([A-J])/);
  if (answerColonMatch?.[1] !== undefined) {
    return answerColonMatch[1];
  }
  const lastStandaloneLetter = cleaned.match(/\b[A-J]\b(?!.*\b[A-J]\b)/s);
  return lastStandaloneLetter?.[0] ?? null;
}
