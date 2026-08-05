import { unknownToString } from "../../internal/errors";
import type { GenerationResult } from "./schemas";

const LEAK_MARKERS: readonly string[] = [
  "perplexity-ai/draco",
  "perplexity-ai___draco",
  "pplx-draco",
  "2602.11685",
];

const SELF_RECOGNITION_RE =
  /\b(?:this|the)\s+(?:prompt|task|question|query|request)\b[^.\n]{0,30}?\b(?:is|are|appears?|looks?|seems?|reads?|resembl\w+|as)\b[^.\n]{0,30}?\b(?:benchmark|rubric|eval(?:uation)?|test)\b[\w/]{0,12}?\s*\b(?:task|item|prompt|question|set|harness|suite)\b/i;

export function detectContamination(
  gen: Pick<GenerationResult, "content" | "fusionAnalysis">
): string[] {
  const haystacks: string[] = [];
  if (gen.content) {
    haystacks.push(gen.content);
  }
  if (gen.fusionAnalysis) {
    haystacks.push(unknownToString(gen.fusionAnalysis));
  }
  const blob = haystacks.join("\n");
  if (blob.length === 0) {
    return [];
  }
  const signals: string[] = [];
  const low = blob.toLowerCase();
  for (const marker of LEAK_MARKERS) {
    if (low.includes(marker.toLowerCase())) {
      signals.push(`leak-marker:${marker}`);
    }
  }
  if (SELF_RECOGNITION_RE.test(blob)) {
    signals.push("benchmark-self-recognition");
  }
  return signals;
}
