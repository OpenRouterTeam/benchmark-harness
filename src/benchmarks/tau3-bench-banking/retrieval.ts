import type { BankingRetrievalConfig } from "./retrieval-config";
import type { ToolDefinition } from "./tools/definitions";

export interface RetrievalDocument {
  readonly id: string;
  readonly title: string;
  readonly content: string;
}

interface IndexedDocument {
  readonly document: RetrievalDocument;
  readonly length: number;
  readonly termFrequencies: ReadonlyMap<string, number>;
}

export interface Bm25Index {
  readonly documents: readonly IndexedDocument[];
  readonly averageDocumentLength: number;
  readonly inverseDocumentFrequencies: ReadonlyMap<string, number>;
}

export interface RetrievalResult {
  readonly document: RetrievalDocument;
  readonly score: number;
}

export interface BankingRetrievalTools {
  readonly definitions: readonly ToolDefinition[];
  readonly invoke: (
    name: string,
    arguments_: Readonly<Record<string, unknown>>
  ) => string | undefined;
}

const BM25_K1 = 1.5;

const BM25_B = 0.75;

const BM25_EPSILON = 0.25;

const DEFAULT_TOP_K = 10;

export const BM25_GREP_TOOL_DEFINITIONS: readonly ToolDefinition[] = [
  {
    type: "function",
    function: {
      name: "KB_search",
      description: "Search the knowledge base for relevant documents.",
      parameters: {
        type: "object",
        properties: {
          query: {
            type: "string",
            description: "The search query to find relevant documents",
          },
        },
        required: ["query"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "grep",
      description:
        "Search for a regex pattern in all knowledge base documents.\n\nReturns documents ranked by number of matches, with full content.",
      parameters: {
        type: "object",
        properties: {
          pattern: {
            type: "string",
            description:
              "The regex pattern to search for (e.g., 'credit.*card', 'fee|charge')",
          },
        },
        required: ["pattern"],
      },
    },
  },
];

export function makeBm25Index(
  documents: readonly RetrievalDocument[]
): Bm25Index {
  const tokenizedDocuments = documents.map((document) =>
    tokenize(document.content)
  );
  const indexedDocuments = documents.map((document, index): IndexedDocument => {
    const tokens = tokenizedDocuments[index] ?? [];
    return {
      document,
      length: tokens.length,
      termFrequencies: countTerms(tokens),
    };
  });
  const totalLength = indexedDocuments.reduce(
    (total, document) => total + document.length,
    0
  );
  const averageDocumentLength =
    indexedDocuments.length === 0 ? 0 : totalLength / indexedDocuments.length;
  const documentFrequencies = countDocumentFrequencies(tokenizedDocuments);
  const rawIdfs = new Map(
    [...documentFrequencies].map(([term, frequency]) => [
      term,
      Math.log(documents.length - frequency + 0.5) - Math.log(frequency + 0.5),
    ])
  );
  const idfSum = [...rawIdfs.values()].reduce((sum, idf) => sum + idf, 0);
  const averageIdf = rawIdfs.size === 0 ? 0 : idfSum / rawIdfs.size;
  const epsilonIdf = BM25_EPSILON * averageIdf;
  const inverseDocumentFrequencies = new Map(
    [...rawIdfs].map(([term, idf]) => [term, idf < 0 ? epsilonIdf : idf])
  );
  return {
    documents: indexedDocuments,
    averageDocumentLength,
    inverseDocumentFrequencies,
  };
}

export function searchBm25({
  index,
  query,
  topK = DEFAULT_TOP_K,
}: {
  readonly index: Bm25Index;
  readonly query: string;
  readonly topK?: number;
}): RetrievalResult[] {
  const queryTokens = tokenize(query);
  if (queryTokens.length === 0) {
    return [];
  }
  return index.documents
    .map((document, position) => ({
      document: document.document,
      score: scoreBm25Document({ document, index, queryTokens }),
      position,
    }))
    .toSorted(
      (left, right) =>
        right.score - left.score || left.position - right.position
    )
    .slice(0, Math.min(topK, index.documents.length))
    .map(({ document, score }) => ({ document, score }));
}

function scoreBm25Document({
  document,
  index,
  queryTokens,
}: {
  readonly document: IndexedDocument;
  readonly index: Bm25Index;
  readonly queryTokens: readonly string[];
}): number {
  if (index.averageDocumentLength === 0) {
    return 0;
  }
  let score = 0;
  for (const token of queryTokens) {
    const frequency = document.termFrequencies.get(token) ?? 0;
    const idf = index.inverseDocumentFrequencies.get(token) ?? 0;
    const numerator = frequency * (BM25_K1 + 1);
    const lengthRatio = document.length / index.averageDocumentLength;
    const denominator =
      frequency + BM25_K1 * (1 - BM25_B + BM25_B * lengthRatio);
    score += idf * (numerator / denominator);
  }
  return score;
}

export function searchGrep({
  documents,
  pattern,
  topK = DEFAULT_TOP_K,
}: {
  readonly documents: readonly RetrievalDocument[];
  readonly pattern: string;
  readonly topK?: number;
}): RetrievalResult[] {
  if (pattern.trim().length === 0) {
    return [];
  }
  const regex = compileGrepPattern(pattern);
  return documents
    .map((document, position) => ({
      document,
      score: [...document.content.matchAll(regex)].length,
      position,
    }))
    .filter((result) => result.score > 0)
    .toSorted(
      (left, right) =>
        right.score - left.score || left.position - right.position
    )
    .slice(0, topK)
    .map(({ document, score }) => ({ document, score }));
}

function compileGrepPattern(pattern: string): RegExp {
  try {
    return new RegExp(pattern, "giu");
  } catch {
    return new RegExp(escapeRegex(pattern), "giu");
  }
}

function escapeRegex(value: string): string {
  return value.replaceAll(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

export function formatKbSearchResults({
  results,
  retrievalMs,
}: {
  readonly results: readonly RetrievalResult[];
  readonly retrievalMs: number;
}): string {
  const timing = `[Timing: retrieval=${retrievalMs.toFixed(0)}ms, total=${retrievalMs.toFixed(0)}ms]`;
  if (results.length === 0) {
    return `No relevant documents found.\n\n${timing}`;
  }
  return `${formatResults(results)}\n\n${timing}`;
}

export function formatGrepResults({
  results,
  pattern,
}: {
  readonly results: readonly RetrievalResult[];
  readonly pattern: string;
}): string {
  if (results.length === 0) {
    return `No matches found for pattern: ${pattern}`;
  }
  return formatResults(results);
}

export function makeBm25GrepRetrievalTools(
  documents: readonly RetrievalDocument[]
): BankingRetrievalTools {
  const index = makeBm25Index(documents);
  return {
    definitions: BM25_GREP_TOOL_DEFINITIONS,
    invoke: (name, arguments_) => {
      if (name === "KB_search") {
        return invokeKbSearch({ arguments_, index });
      }
      if (name === "grep") {
        return invokeGrep({ arguments_, documents });
      }
      return undefined;
    },
  };
}

export function makeBankingRetrievalTools({
  documents,
  retrievalConfig,
}: {
  readonly documents: readonly RetrievalDocument[];
  readonly retrievalConfig: BankingRetrievalConfig;
}): BankingRetrievalTools {
  switch (retrievalConfig) {
    case "required_docs": {
      return { definitions: [], invoke: () => undefined };
    }
    case "bm25_grep": {
      return makeBm25GrepRetrievalTools(documents);
    }
    default: {
      retrievalConfig satisfies never;
      return { definitions: [], invoke: () => undefined };
    }
  }
}

function invokeKbSearch({
  arguments_,
  index,
}: {
  readonly arguments_: Readonly<Record<string, unknown>>;
  readonly index: Bm25Index;
}): string {
  const query = arguments_["query"];
  if (typeof query !== "string") {
    return "Error: Missing required parameter: query";
  }
  const start = performance.now();
  const results = searchBm25({ index, query });
  return formatKbSearchResults({
    results,
    retrievalMs: performance.now() - start,
  });
}

function invokeGrep({
  arguments_,
  documents,
}: {
  readonly arguments_: Readonly<Record<string, unknown>>;
  readonly documents: readonly RetrievalDocument[];
}): string {
  const pattern = arguments_["pattern"];
  if (typeof pattern !== "string") {
    return "Error: Missing required parameter: pattern";
  }
  return formatGrepResults({
    results: searchGrep({ documents, pattern }),
    pattern,
  });
}

function formatResults(results: readonly RetrievalResult[]): string {
  return results
    .map(
      ({ document, score }, index) =>
        `${index + 1}. ${document.title}\n` +
        `   ID: ${document.id}\n` +
        `   Score: ${score.toFixed(4)}\n` +
        `   Content: ${document.content}\n`
    )
    .join("\n");
}

function tokenize(text: string): string[] {
  const normalized = text.toLowerCase().trim();
  return normalized.length === 0 ? [] : normalized.split(/\s+/u);
}

function countTerms(tokens: readonly string[]): ReadonlyMap<string, number> {
  const frequencies = new Map<string, number>();
  for (const token of tokens) {
    frequencies.set(token, (frequencies.get(token) ?? 0) + 1);
  }
  return frequencies;
}

function countDocumentFrequencies(
  documents: readonly (readonly string[])[]
): ReadonlyMap<string, number> {
  const frequencies = new Map<string, number>();
  for (const tokens of documents) {
    for (const token of new Set(tokens)) {
      frequencies.set(token, (frequencies.get(token) ?? 0) + 1);
    }
  }
  return frequencies;
}
