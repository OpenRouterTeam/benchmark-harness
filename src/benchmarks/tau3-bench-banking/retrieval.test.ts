import { describe, expect, it } from "bun:test";

import {
  formatGrepResults,
  formatKbSearchResults,
  makeBankingRetrievalTools,
  makeBm25Index,
  searchBm25,
  searchGrep,
} from "./retrieval";

const DOCUMENTS = [
  {
    id: "doc_checking",
    title: "Checking Accounts",
    content: "checking account overdraft fee checking",
  },
  {
    id: "doc_credit",
    title: "Credit Cards",
    content: "credit card annual fee and rewards",
  },
  {
    id: "doc_disputes",
    title: "Card Disputes",
    content: "credit card dispute chargeback chargeback",
  },
] as const;
describe("tau3 banking retrieval", () => {
  it("ranks BM25 documents with upstream whitespace tokenization", () => {
    const index = makeBm25Index(DOCUMENTS);
    const results = searchBm25({ index, query: "annual rewards", topK: 2 });
    expect(results.map((result) => result.document.id)).toEqual([
      "doc_credit",
      "doc_checking",
    ]);
    expect(results[0]?.score).toBeGreaterThan(0);
    expect(results[1]?.score).toBe(0);
  });
  it("keeps punctuation attached to tokens like rank-bm25 0.2.2", () => {
    const index = makeBm25Index(DOCUMENTS);
    const results = searchBm25({ index, query: "fee,", topK: 1 });
    expect(results[0]?.document.id).toBe("doc_checking");
    expect(results[0]?.score).toBe(0);
  });
  it("formats KB_search results with upstream labels and timing", () => {
    const index = makeBm25Index(DOCUMENTS);
    const results = searchBm25({ index, query: "annual rewards", topK: 1 });
    const formatted = formatKbSearchResults({ results, retrievalMs: 12.4 });
    expect(formatted).toMatchInlineSnapshot(`
      "1. Credit Cards
         ID: doc_credit
         Score: 0.9672
         Content: credit card annual fee and rewards


      [Timing: retrieval=12ms, total=12ms]"
    `);
  });
  it("ranks grep matches by count and formats full documents", () => {
    const results = searchGrep({
      documents: DOCUMENTS,
      pattern: "chargeback|annual",
    });
    expect(results.map((result) => result.document.id)).toEqual([
      "doc_disputes",
      "doc_credit",
    ]);
    const formatted = formatGrepResults({
      results,
      pattern: "chargeback|annual",
    });
    expect(formatted).toContain("Score: 2.0000");
    expect(formatted).toContain("Content: credit card dispute");
  });
  it("falls back to a literal grep pattern when the regex is invalid", () => {
    const documents = [
      {
        id: "doc_literal",
        title: "Literal",
        content: "Call support (option one).",
      },
    ];
    const results = searchGrep({ documents, pattern: "(" });
    expect(results).toHaveLength(1);
    expect(results[0]?.score).toBe(1);
  });
  it("uses upstream empty-result strings", () => {
    expect(formatKbSearchResults({ results: [], retrievalMs: 0 })).toBe(
      "No relevant documents found.\n\n[Timing: retrieval=0ms, total=0ms]"
    );
    expect(formatGrepResults({ results: [], pattern: "missing" })).toBe(
      "No matches found for pattern: missing"
    );
  });
  it("exposes retrieval tools only for bm25_grep", () => {
    const goldenTools = makeBankingRetrievalTools({
      documents: DOCUMENTS,
      retrievalConfig: "required_docs",
    });
    const bm25Tools = makeBankingRetrievalTools({
      documents: DOCUMENTS,
      retrievalConfig: "bm25_grep",
    });
    expect(goldenTools.definitions).toEqual([]);
    expect(bm25Tools.definitions.map((tool) => tool.function.name)).toEqual([
      "KB_search",
      "grep",
    ]);
    expect(bm25Tools.definitions[1]?.function.description).toBe(
      "Search for a regex pattern in all knowledge base documents.\n\nReturns documents ranked by number of matches, with full content."
    );
  });
});
