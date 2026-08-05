export interface FencedBlock {
  readonly block: string | null;
  readonly openingFence: string | null;
}

export interface CodeBlockUsage {
  readonly usesCodeBlock: boolean;
  readonly codeBlockType: string | null;
}

export interface CommentaryCheck {
  readonly hasCommentary: boolean;
  readonly description: string | null;
}

export function removeThinkingTags(text: string): string {
  const withoutPairs = text.replaceAll(
    /<think>.*?<\/think>|\[THINK\].*?\[\/THINK\]/gis,
    ""
  );
  const withoutLeading = withoutPairs.replaceAll(
    /^.*?<\/think>|^.*?\[\/THINK\]/gis,
    ""
  );
  const withoutTrailing = withoutLeading.replaceAll(
    /<think>.*$|\[THINK\].*$/gis,
    ""
  );
  const harmony =
    /(?:<\|start\|>)?assistant(?:<\|channel\|>)?final(?:<\|message\|>)?/.exec(
      withoutTrailing
    );
  const afterHarmony =
    harmony !== null
      ? withoutTrailing.slice(harmony.index + harmony[0].length)
      : withoutTrailing;
  return afterHarmony.replace(/<\|(?:end|return)\|>\s*$/, "").trim();
}

export function extractOuterFencedBlock(response: string): FencedBlock {
  const lines = response.trim().split("\n");
  const startIdx = lines.findIndex((line) => line.startsWith("```"));
  if (startIdx === -1) {
    return { block: null, openingFence: null };
  }
  const openingFence = lines[startIdx] ?? null;
  const closeOffset = lines
    .slice(startIdx + 1)
    .findIndex((line) => line.startsWith("```"));
  if (closeOffset === -1) {
    return { block: null, openingFence };
  }
  const closeIdx = startIdx + 1 + closeOffset;
  return {
    block: lines.slice(startIdx + 1, closeIdx).join("\n"),
    openingFence,
  };
}

export function checkUsesCodeBlock(response: string): CodeBlockUsage {
  const { block, openingFence } = extractOuterFencedBlock(response.trim());
  if (block === null || openingFence === null) {
    return { usesCodeBlock: false, codeBlockType: null };
  }
  const lowered = openingFence.trim().toLowerCase();
  if (lowered.startsWith("```yaml")) {
    return { usesCodeBlock: true, codeBlockType: "```yaml" };
  }
  if (lowered.startsWith("```yml")) {
    return { usesCodeBlock: true, codeBlockType: "```yml" };
  }
  if (lowered.startsWith("```json")) {
    return { usesCodeBlock: true, codeBlockType: "```json" };
  }
  return { usesCodeBlock: true, codeBlockType: "```" };
}

export function findJsonBoundaries(
  response: string
): readonly [number, number] | null {
  const firstBrace = response.indexOf("{");
  const firstBracket = response.indexOf("[");
  if (firstBrace === -1 && firstBracket === -1) {
    return null;
  }
  const bracesFirst =
    firstBracket === -1 || (firstBrace !== -1 && firstBrace < firstBracket);
  const charOrder: readonly (readonly [string, string])[] = bracesFirst
    ? [
        ["{", "}"],
        ["[", "]"],
      ]
    : [
        ["[", "]"],
        ["{", "}"],
      ];
  for (const [startChar, endChar] of charOrder) {
    const startIdx = response.indexOf(startChar);
    if (startIdx === -1) {
      continue;
    }
    const end = matchJsonContainer({
      text: response,
      start: startIdx,
      open: startChar,
      close: endChar,
    });
    if (end !== null) {
      return [startIdx, end - 1];
    }
  }
  return null;
}

export function checkForCommentary(response: string): CommentaryCheck {
  const trimmed = response.trim();
  const { block } = extractOuterFencedBlock(trimmed);
  const remaining =
    block !== null
      ? remainingAroundFence(trimmed)
      : remainingAroundJson(trimmed);
  if (remaining === null || remaining === "") {
    return { hasCommentary: false, description: null };
  }
  return {
    hasCommentary: true,
    description: `Response contains text outside JSON: "${preview(remaining)}"`,
  };
}

export function checkForCommentaryYaml(response: string): CommentaryCheck {
  const trimmed = response.trim();
  const { block } = extractOuterFencedBlock(trimmed);
  if (block === null) {
    return { hasCommentary: false, description: null };
  }
  const remaining = remainingAroundFence(trimmed);
  if (remaining === "") {
    return { hasCommentary: false, description: null };
  }
  return {
    hasCommentary: true,
    description: `Response contains text outside YAML: "${preview(remaining)}"`,
  };
}

function remainingAroundFence(response: string): string {
  const before = response.slice(0, response.indexOf("```")).trim();
  const endMarker = response.lastIndexOf("```");
  const after = endMarker !== -1 ? response.slice(endMarker + 3).trim() : "";
  return `${before} ${after}`.trim();
}

function remainingAroundJson(response: string): string | null {
  const boundaries = findJsonBoundaries(response);
  if (boundaries === null) {
    return null;
  }
  const [startIdx, endIdx] = boundaries;
  const before = response.slice(0, startIdx).trim();
  const after = response.slice(endIdx + 1).trim();
  return `${before} ${after}`.trim();
}

function preview(remaining: string): string {
  return remaining.length > 100 ? `${remaining.slice(0, 100)}...` : remaining;
}

export function matchJsonContainer(args: {
  text: string;
  start: number;
  open: string;
  close: string;
}): number | null {
  const { text, start, open, close } = args;
  let depth = 0;
  let inString = false;
  let escapeNext = false;
  for (let i = start; i < text.length; i++) {
    const c = text[i];
    if (escapeNext) {
      escapeNext = false;
      continue;
    }
    if (c === "\\") {
      escapeNext = true;
      continue;
    }
    if (c === '"') {
      inString = !inString;
      continue;
    }
    if (inString) {
      continue;
    }
    if (c === open) {
      depth += 1;
    } else if (c === close) {
      depth -= 1;
      if (depth === 0) {
        return i + 1;
      }
    }
  }
  return null;
}
