const CODE_DIGIT_0 = 48;
const CODE_DIGIT_9 = 57;
const CODE_UPPER_A = 65;
const CODE_UPPER_Z = 90;
const CODE_LOWER_A = 97;
const CODE_LOWER_Z = 122;
const ASCII_LIMIT = 128;
const CASE_OFFSET = 32;

const UPPER_ASCII_LETTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const LOWER_ASCII_LETTERS = "abcdefghijklmnopqrstuvwxyz";

const IS_ASCII_CASE_LOCALE_NEUTRAL =
  UPPER_ASCII_LETTERS.toLocaleLowerCase() === LOWER_ASCII_LETTERS &&
  LOWER_ASCII_LETTERS.toLocaleUpperCase() === UPPER_ASCII_LETTERS;

const WORD_RUN = /[\p{L}\d]+/giu;
const WORD_BOUNDARY = /(?<=[\p{Ll}\d])(?=\p{Lu})|(?<=\p{Lu})(?=\p{Lu}\p{Ll})/u;

export function snakeCase(input: string): string {
  const words = IS_ASCII_CASE_LOCALE_NEUTRAL ? asciiWords(input) : undefined;
  if (words !== undefined) {
    return words.join("_").toLowerCase();
  }
  return unicodeWords(input)
    .map((word) => word.toLocaleLowerCase())
    .join("_");
}

export function camelCase(input: string): string {
  const words = IS_ASCII_CASE_LOCALE_NEUTRAL ? asciiWords(input) : undefined;
  if (words !== undefined) {
    return asciiCamelJoin(words);
  }
  return unicodeCamelJoin(unicodeWords(input));
}

function isAsciiLower(code: number): boolean {
  return code >= CODE_LOWER_A && code <= CODE_LOWER_Z;
}

function isAsciiUpper(code: number): boolean {
  return code >= CODE_UPPER_A && code <= CODE_UPPER_Z;
}

function isAsciiDigit(code: number): boolean {
  return code >= CODE_DIGIT_0 && code <= CODE_DIGIT_9;
}

function startsUpperWord(input: string, index: number): boolean {
  const previous = input.charCodeAt(index - 1);
  if (isAsciiLower(previous) || isAsciiDigit(previous)) {
    return true;
  }
  return isAsciiUpper(previous) && isAsciiLower(input.charCodeAt(index + 1));
}

function asciiWords(input: string): string[] | undefined {
  const words: string[] = [];
  let start = -1;
  for (let index = 0; index < input.length; index++) {
    const code = input.charCodeAt(index);
    if (code >= ASCII_LIMIT) {
      return undefined;
    }
    if (!(isAsciiLower(code) || isAsciiUpper(code) || isAsciiDigit(code))) {
      if (start >= 0) {
        words.push(input.slice(start, index));
        start = -1;
      }
      continue;
    }
    if (start < 0) {
      start = index;
      continue;
    }
    if (isAsciiUpper(code) && startsUpperWord(input, index)) {
      words.push(input.slice(start, index));
      start = index;
    }
  }
  if (start >= 0) {
    words.push(input.slice(start));
  }
  return words;
}

function asciiCamelJoin(words: readonly string[]): string {
  if (words.length === 0) {
    return "";
  }
  let output = words[0]!.toLowerCase();
  for (let index = 1; index < words.length; index++) {
    const word = words[index]!;
    const code = word.charCodeAt(0);
    const rest = word.slice(1).toLowerCase();
    if (isAsciiDigit(code)) {
      output += `_${word[0]}${rest}`;
    } else if (isAsciiLower(code)) {
      output += String.fromCharCode(code - CASE_OFFSET) + rest;
    } else {
      output += word[0] + rest;
    }
  }
  return output;
}

function unicodeWords(input: string): string[] {
  const runs = input.match(WORD_RUN);
  if (runs === null) {
    return [];
  }
  return runs.flatMap((run) => run.split(WORD_BOUNDARY));
}

function unicodeCamelJoin(words: readonly string[]): string {
  return words
    .map((word, index) => {
      if (index === 0) {
        return word.toLocaleLowerCase();
      }
      const initial = word[0]!;
      const head =
        initial >= "0" && initial <= "9"
          ? `_${initial}`
          : initial.toLocaleUpperCase();
      return head + word.slice(1).toLocaleLowerCase();
    })
    .join("");
}
