import { describe, it, expect, beforeEach } from "bun:test";
import { createHash } from "node:crypto";

import {
  buildBankingAgentSystemPrompt,
  DEFAULT_FIRST_AGENT_MESSAGE,
} from "./agent-prompts";
import { seedBankingDocumentsCache } from "./documents";

const FIXTURE_DOCS = [
  {
    id: "doc_test_accounts_001",
    title: "Accounts Overview",
    content: "Information about bank accounts.",
  },
  {
    id: "doc_test_cards_002",
    title: "Debit Cards Guide",
    content: "How to use debit cards.",
  },
  {
    id: "doc_test_disputes_003",
    title: "Dispute Resolution Process",
    content: "Steps to file a dispute.",
  },
];
describe("Banking Agent Prompts", () => {
  beforeEach(() => {
    seedBankingDocumentsCache(FIXTURE_DOCS);
  });
  describe("DEFAULT_FIRST_AGENT_MESSAGE", () => {
    it("is a greeting string", () => {
      expect(typeof DEFAULT_FIRST_AGENT_MESSAGE).toBe("string");
      expect(DEFAULT_FIRST_AGENT_MESSAGE.length).toBeGreaterThan(0);
      expect(DEFAULT_FIRST_AGENT_MESSAGE).toContain("help");
    });
  });
  describe("buildBankingAgentSystemPrompt", () => {
    it("builds prompt with single required document", () => {
      const prompt = buildBankingAgentSystemPrompt({
        requiredDocIds: ["doc_test_accounts_001"],
        retrievalConfig: "required_docs",
      });
      expect(prompt).toContain("<instructions>");
      expect(prompt).toContain("</instructions>");
      expect(prompt).toContain("<policy>");
      expect(prompt).toContain("</policy>");
      expect(prompt).toContain("# Rho-Bank Customer Service Policy");
      expect(prompt).toContain("## Accounts Overview");
      expect(prompt).toContain("Information about bank accounts.");
      expect(prompt).toContain("<required_documents>");
    });
    it("renders multiple required documents in order", () => {
      const prompt = buildBankingAgentSystemPrompt({
        requiredDocIds: [
          "doc_test_accounts_001",
          "doc_test_cards_002",
          "doc_test_disputes_003",
        ],
        retrievalConfig: "required_docs",
      });
      expect(prompt).toContain("## Accounts Overview");
      expect(prompt).toContain("## Debit Cards Guide");
      expect(prompt).toContain("## Dispute Resolution Process");
      const accountsIdx = prompt.indexOf("## Accounts Overview");
      const cardsIdx = prompt.indexOf("## Debit Cards Guide");
      const disputesIdx = prompt.indexOf("## Dispute Resolution Process");
      expect(accountsIdx).toBeLessThan(cardsIdx);
      expect(cardsIdx).toBeLessThan(disputesIdx);
    });
    it("renders documents separated by --- separator", () => {
      const prompt = buildBankingAgentSystemPrompt({
        requiredDocIds: ["doc_test_accounts_001", "doc_test_cards_002"],
        retrievalConfig: "required_docs",
      });
      expect(prompt).toContain("---");
    });
    it("renders zero-doc variant as no documents provided", () => {
      const prompt = buildBankingAgentSystemPrompt({
        requiredDocIds: [],
        retrievalConfig: "required_docs",
      });
      expect(prompt).toContain("(No documents provided)");
    });
    it("includes policy header component", () => {
      const prompt = buildBankingAgentSystemPrompt({
        requiredDocIds: ["doc_test_accounts_001"],
        retrievalConfig: "required_docs",
      });
      expect(prompt).toContain("# Rho-Bank Customer Service Policy");
      expect(prompt).toContain("You are a helpful customer service agent");
      expect(prompt).toContain("## Guidelines");
    });
    it("includes additional instructions component", () => {
      const prompt = buildBankingAgentSystemPrompt({
        requiredDocIds: ["doc_test_accounts_001"],
        retrievalConfig: "required_docs",
      });
      expect(prompt).toContain("## Additional Instructions");
      expect(prompt).toContain("### Discoverable Tools");
      expect(prompt).toContain("### Authenticating Users");
    });
    it("throws when required doc id not in cache", () => {
      const [firstDoc] = FIXTURE_DOCS;
      if (!firstDoc) {
        throw new Error("FIXTURE_DOCS is empty");
      }
      seedBankingDocumentsCache([firstDoc]);
      expect(() =>
        buildBankingAgentSystemPrompt({
          requiredDocIds: ["missing_doc_id"],
          retrievalConfig: "required_docs",
        })
      ).toThrow(/not in cache/);
    });
    it("throws with doc id in error message", () => {
      seedBankingDocumentsCache([]);
      expect(() =>
        buildBankingAgentSystemPrompt({
          requiredDocIds: ["specific_missing_doc_id"],
          retrievalConfig: "required_docs",
        })
      ).toThrow(/specific_missing_doc_id/);
    });
    it("wraps prompt with instructions and policy tags", () => {
      const prompt = buildBankingAgentSystemPrompt({
        requiredDocIds: ["doc_test_accounts_001"],
        retrievalConfig: "required_docs",
      });
      const instructionsStart = prompt.indexOf("<instructions>");
      const instructionsEnd = prompt.indexOf("</instructions>");
      const hasInstructionsTag = prompt.includes("<instructions>");
      const hasInstructionsCloseTag = prompt.includes("</instructions>");
      const hasPolicyTag = prompt.includes("<policy>");
      const hasPolicyCloseTag = prompt.includes("</policy>");
      expect(instructionsStart).toBeGreaterThanOrEqual(0);
      expect(instructionsEnd).toBeGreaterThan(instructionsStart);
      expect(hasInstructionsTag).toBe(true);
      expect(hasInstructionsCloseTag).toBe(true);
      expect(hasPolicyTag).toBe(true);
      expect(hasPolicyCloseTag).toBe(true);
    });
    it("uses the upstream classic BM25 and grep prompt without golden documents", () => {
      const prompt = buildBankingAgentSystemPrompt({
        requiredDocIds: ["doc_test_accounts_001"],
        retrievalConfig: "bm25_grep",
      });
      expect(prompt).toContain(
        "**Search the knowledge base** for relevant information when appropriate using the provided `KB_search` (uses BM25 for retrieval) and `grep` tools."
      );
      expect(prompt).not.toContain("<required_documents>");
      expect(prompt).not.toContain("Information about bank accounts.");
    });
    it("matches the pinned upstream bm25_grep system prompt byte-for-byte", () => {
      const prompt = buildBankingAgentSystemPrompt({
        requiredDocIds: [],
        retrievalConfig: "bm25_grep",
      });
      expect(createHash("sha256").update(prompt).digest("hex")).toBe(
        "6106f260b43df9b84e22ad54cc8d6652ce8eee9831fef4a208c7d82145d74462"
      );
    });
    it("matches the pinned required_docs system prompt byte-for-byte", () => {
      const prompt = buildBankingAgentSystemPrompt({
        requiredDocIds: [],
        retrievalConfig: "required_docs",
      });
      expect(createHash("sha256").update(prompt).digest("hex")).toBe(
        "49d311c73c3034fb255b5d4967e598959bde63908dc8c4fa101e94ab57e78a60"
      );
    });
  });
});
