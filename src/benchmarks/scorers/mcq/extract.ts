const ASCII_UPPERCASE = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

export function stripMdLatex(response: string): string {
  return response
    .replaceAll("**", "")
    .replaceAll("$\\boxed{", "")
    .replaceAll("}$", "")
    .replaceAll("\\$", "")
    .replaceAll("$\\text{", "")
    .replaceAll("$", "")
    .replaceAll("\\mathrm{", "")
    .replaceAll("\\{", "")
    .replaceAll("\\text", "")
    .replaceAll("\\(", "")
    .replaceAll("\\mathbf{", "")
    .replaceAll("{", "")
    .replaceAll("\\boxed", "");
}

export function normalizeMcqAnswer(extractedAnswer: string): string {
  return extractedAnswer
    .replaceAll("أ", " A")
    .replaceAll("ب", " B")
    .replaceAll("ج", " C")
    .replaceAll("د", " D")
    .replaceAll("অ", " A")
    .replaceAll("ব", " B")
    .replaceAll("ড", " C")
    .replaceAll("ঢ", " D")
    .replaceAll("Ａ", " A")
    .replaceAll("Ｂ", " B")
    .replaceAll("Ｃ", " C")
    .replaceAll("Ｄ", " D")
    .replaceAll("Ｅ", " E")
    .replaceAll("Ｆ", " F")
    .replaceAll("Ｇ", " G")
    .replaceAll("Ｈ", " H")
    .replaceAll("Ｉ", " I")
    .replaceAll("Ｊ", " J")
    .trim();
}

const MCQ_PATTERNS: readonly RegExp[] = [
  /(?:\*{1,2}|_{1,2})Answer[s]?\s*[:\-–]?(?:\*{1,2}|_{1,2})\s*([A-Z])\b/i,
  /^\s*(?:\*{1,2}|_{1,2})?Answer:?(?:\*{1,2}|_{1,2})?\s*:?\s*(?:\*{1,2}|_{1,2})?([A-Z])(?:\*{1,2}|_{1,2})?\s*/im,
  /\bAnswer[s]?\b\s*[:\-–]?\s*\(\s*([A-Z])\s*\)/i,
  /\bAnswer[s]?\b\s*[:\-–]?\s*([A-Z])\b/i,
  /\b(?:Option|Choice)\b\s*[:\-–]?\s*([A-Z])\b/i,
  /\\boxed\{[^}]*?([A-Z])[^}]*\}/m,
  /\\boxed\{[^}]*?\\textbf\{[^}]*?([A-Z])[^}]*\}[^}]*\}/m,
  /\\boxed\{[^}]*?\\text\{[^}]*?([A-Z])[^}]*\}[^}]*\}/m,
  /(?<![A-Za-z0-9])[([]\s*([A-Z])\s*[)\]](?![A-Za-z0-9])/,
  /(?<![A-Za-z0-9])(?:\*{1,2}|_{1,2})([A-Z])(?:\*{1,2}|_{1,2})(?![A-Za-z0-9])/,
  /\\textbf\{[^}]*?([A-Z])[^}]*\}/,
  /(?<![A-Za-z0-9])(?:\*{1,2}|_{1,2})\s*([A-Z])\)[^*_\n]+?(?:\*{1,2}|_{1,2})(?![A-Za-z0-9])/,
  /^\s*(?:\*{1,2}|_{1,2})?([A-Z])(?:\*{1,2}|_{1,2})?\s*[.)\-–:]?\s*$/m,
];

const MULTILINGUAL_ANSWER_REGEXES: readonly string[] = [
  "Answer\\s*:",
  "Answer\\s*:​​​​​​",
  "উত্তর\\s*:",
  "उत्तर\\s*:",
  "উত্তরঃ",
  "উত্তর\\s*:",
  "Antwort\\s*:",
  "답변\\s*:",
  "정답\\s*:",
  "답\\s*:",
  "答案\\s*：",
  "答案\\s*:",
  "答\\s*：",
  "答\\s*:",
  "答复\\s*：",
  "答曰\\s*：",
  "الإجابة:",
  "الجواب:",
  "إجابة:",
  "الإجابة النهائية:",
  "الإجابة الصحيحة:",
  "الإجابة الصحيحة هي:",
  "الإجابة هي:",
  "الجواب النهائي:",
  "Respuesta\\s*:",
  "Risposta\\s*:",
  "答え\\s*:",
  "答え\\s*：",
  "回答\\s*:",
  "回答\\s*：",
  "解答\\s*:",
  "Jawaban\\s*:",
  "Javob\\s*:",
  "Жавоб\\s*:",
  "Cevap\\s*:",
  "Джевап\\s*:",
  "Җавап\\s*:",
  "Жауап\\s*:",
  "Jawap\\s*:",
  "Juwap\\s*:",
  "جاۋاب\\:",
  "Cavab\\s*:",
  "Réponse\\s*:",
  "Resposta\\s*:",
  "Jibu\\s*:",
  "Idahun\\s*:",
];

const MULTILINGUAL_ANSWER_PATTERN_TEMPLATE =
  "(?i){}[ \\t]*([A-J]|[أ-د]|[অ]|[ব]|[ড]|[ঢ]|[Ａ-Ｊ])";

const MULTILINGUAL_PATTERNS: readonly RegExp[] =
  MULTILINGUAL_ANSWER_REGEXES.map((answerRegex) => {
    const withFlag = MULTILINGUAL_ANSWER_PATTERN_TEMPLATE.replace(
      "{}",
      answerRegex
    );
    const body = withFlag.startsWith("(?i)") ? withFlag.slice(4) : withFlag;
    return new RegExp(body, "i");
  });

interface CandidateMatch {
  readonly priority: number;
  readonly matchLength: number;
  readonly letter: string;
}

export function extractMcqAnswer(text: string): string | null {
  if (!text) {
    return null;
  }
  const cleanedText = stripMdLatex(text);
  const matches: CandidateMatch[] = [];
  for (let priority = 0; priority < MCQ_PATTERNS.length; priority++) {
    const match = MCQ_PATTERNS[priority]!.exec(text);
    if (match?.[1]) {
      const letter = match[1].toUpperCase();
      if (letter.length === 1 && ASCII_UPPERCASE.includes(letter)) {
        matches.push({ priority, matchLength: match[0].length, letter });
      }
    }
  }
  for (let idx = 0; idx < MULTILINGUAL_PATTERNS.length; idx++) {
    const match = MULTILINGUAL_PATTERNS[idx]!.exec(cleanedText);
    if (match?.[1]) {
      const normalized = normalizeMcqAnswer(match[1]).toUpperCase();
      if (normalized.length === 1 && ASCII_UPPERCASE.includes(normalized)) {
        matches.push({
          priority: MCQ_PATTERNS.length + idx,
          matchLength: match[0].length,
          letter: normalized,
        });
      }
    }
  }
  if (matches.length === 0) {
    return null;
  }
  matches.sort(
    (a, b) => a.priority - b.priority || b.matchLength - a.matchLength
  );
  return matches[0]!.letter;
}
