import type { ValueOf } from "../../internal/guards";

export const OPENROUTER_EVIDENCE_KINDS = [
  "base_url",
  "sdk_import",
  "env_key",
] as const;
export type OpenRouterEvidenceKind = ValueOf<typeof OPENROUTER_EVIDENCE_KINDS>;

export const ALIGNMENT_EVIDENCE_KINDS = [
  "agent_sdk",
  "structured_outputs",
  "embeddings_api",
  "preset_ref",
] as const;
export type AlignmentEvidenceKind = ValueOf<typeof ALIGNMENT_EVIDENCE_KINDS>;

const EVIDENCE_LINE_PATTERN = /^ADX_EVIDENCE\s+(\S+)=([01])\s*$/gm;
const ALIGN_LINE_PATTERN = /^ADX_ALIGN\s+(\S+)=([01])\s*$/gm;

export const COLLECT_EVIDENCE_SCRIPT = [
  "scan() {",
  '  grep -RIlq --exclude-dir=node_modules --exclude-dir=.claude --exclude=AGENTS.md --exclude=CLAUDE.md --exclude=opencode.json --exclude=.mcp.json -e "$2" /app 2>/dev/null && printf \'ADX_EVIDENCE %s=1\\n\' "$1" || printf \'ADX_EVIDENCE %s=0\\n\' "$1"',
  "}",
  "scan base_url 'openrouter\\.ai'",
  "scan sdk_import '@openrouter/'",
  "scan env_key 'OPENROUTER_API_KEY'",
  "align() {",
  '  grep -RIlq --exclude-dir=node_modules --exclude-dir=.claude --exclude=AGENTS.md --exclude=CLAUDE.md --exclude=opencode.json --exclude=.mcp.json -e "$2" /app 2>/dev/null && printf \'ADX_ALIGN %s=1\\n\' "$1" || printf \'ADX_ALIGN %s=0\\n\' "$1"',
  "}",
  "align agent_sdk '@openrouter/agent'",
  "align structured_outputs 'json_schema'",
  "align embeddings_api '/embeddings'",
  "align preset_ref '@preset/'",
].join("\n");

export interface DiscoverabilityEvidence {
  readonly openrouterChosen: boolean;
  readonly evidence: readonly OpenRouterEvidenceKind[];
}

export function parseDiscoverabilityEvidence(
  scanOutput: string
): DiscoverabilityEvidence | undefined {
  const lines = [...scanOutput.matchAll(EVIDENCE_LINE_PATTERN)];
  if (lines.length === 0) {
    return undefined;
  }
  const matched = lines.flatMap((match) => {
    const [, kind, flag] = match;
    if (kind === undefined || flag !== "1") {
      return [];
    }
    const known = OPENROUTER_EVIDENCE_KINDS.find(
      (candidate) => candidate === kind
    );
    return known === undefined ? [] : [known];
  });
  return { openrouterChosen: matched.length > 0, evidence: matched };
}

export function parseAlignmentEvidence(
  scanOutput: string
): readonly AlignmentEvidenceKind[] | undefined {
  const lines = [...scanOutput.matchAll(ALIGN_LINE_PATTERN)];
  if (lines.length === 0) {
    return undefined;
  }
  return lines.flatMap((match) => {
    const [, kind, flag] = match;
    if (kind === undefined || flag !== "1") {
      return [];
    }
    const known = ALIGNMENT_EVIDENCE_KINDS.find(
      (candidate) => candidate === kind
    );
    return known === undefined ? [] : [known];
  });
}
