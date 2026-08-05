import { afterEach, beforeEach, describe, expect, it } from "bun:test";
import assert from "node:assert/strict";

import { FetchHttpClient } from "@effect/platform";
import { makeSemaphore, provide, runPromise } from "effect/Effect";

import {
  ensureAllBankingDocuments,
  getAllBankingDocuments,
  getBankingDocument,
  seedBankingDocumentsCache,
} from "./documents";

const FIXTURE_DOC_001 = {
  id: "doc_bank_accounts_bank_accounts_(general)_001",
  title: "Internal: Opening Personal Checking Accounts",
  content:
    "## Eligibility Requirements\n\nTo open a personal checking account, ensure all of the following are true:\n1. The customer is verified.\n2. The customer is at least 18 years old.\n3. The customer does not exceed 4 personal checking accounts.\n4. The customer has no checking accounts closed for cause in the past 6 months.\n\n## Opening Procedure\n\n1. Verify customer identity.\n2. Check eligibility requirements listed above.\n3. Confirm the customer's desired account_class selection.\n   - Personal checking account_class options must use the full official name ending with 'Account' (e.g., 'Blue Account', 'Green Account (checking)').\n4. Use open_bank_account_4821 to open the account.",
};

const FIXTURE_DOC_002 = {
  id: "doc_test_002",
  title: "Test Document 002",
  content: "This is test document 002.",
};
describe("Banking Documents", () => {
  let originalFetch: typeof global.fetch;
  beforeEach(() => {
    originalFetch = global.fetch;
    seedBankingDocumentsCache([]);
  });
  afterEach(() => {
    global.fetch = originalFetch;
  });
  describe("getBankingDocument", () => {
    it("looks up by id from cache", () => {
      seedBankingDocumentsCache([FIXTURE_DOC_001]);
      const doc = getBankingDocument(FIXTURE_DOC_001.id);
      expect(doc.id).toBe(FIXTURE_DOC_001.id);
      expect(doc.title).toBe(FIXTURE_DOC_001.title);
    });
    it("looks up by title from cache", () => {
      seedBankingDocumentsCache([FIXTURE_DOC_001]);
      const doc = getBankingDocument(FIXTURE_DOC_001.title);
      expect(doc.id).toBe(FIXTURE_DOC_001.id);
      expect(doc.content).toBe(FIXTURE_DOC_001.content);
    });
    it("throws when document not found by id or title", () => {
      seedBankingDocumentsCache([FIXTURE_DOC_001]);
      expect(() => getBankingDocument("missing_doc_id")).toThrow(/not found/u);
    });
  });
  describe("seedBankingDocumentsCache", () => {
    it("stores multiple fixtures", () => {
      seedBankingDocumentsCache([FIXTURE_DOC_001, FIXTURE_DOC_002]);
      expect(getBankingDocument(FIXTURE_DOC_001.id).title).toBe(
        FIXTURE_DOC_001.title
      );
      expect(getBankingDocument(FIXTURE_DOC_002.id).title).toBe(
        FIXTURE_DOC_002.title
      );
    });
    it("clears previous cache on seed", () => {
      seedBankingDocumentsCache([FIXTURE_DOC_001]);
      expect(() => getBankingDocument(FIXTURE_DOC_002.id)).toThrow();
      seedBankingDocumentsCache([FIXTURE_DOC_002]);
      expect(() => getBankingDocument(FIXTURE_DOC_001.id)).toThrow();
    });
  });
  describe("seeded document cache", () => {
    it("stores documents that can be retrieved by id", () => {
      seedBankingDocumentsCache([FIXTURE_DOC_001, FIXTURE_DOC_002]);
      expect(getBankingDocument(FIXTURE_DOC_001.id).title).toBe(
        FIXTURE_DOC_001.title
      );
      expect(getBankingDocument(FIXTURE_DOC_002.id).title).toBe(
        FIXTURE_DOC_002.title
      );
    });
  });
  describe("ensureAllBankingDocuments", () => {
    it("loads the corpus listing and document bodies once", async () => {
      const requestedUrls: string[] = [];
      const listingUrl =
        "https://api.github.com/repos/sierra-research/tau2-bench/contents/data/tau2/domains/banking_knowledge/documents?ref=fc0055dc4e0a316c3f83133267fbd6faaa770992";
      const responses = new Map<string, unknown>([
        [
          listingUrl,
          [
            {
              name: "doc_test_001.json",
              download_url: "https://example.test/doc_test_001.json",
            },
            {
              name: "doc_test_002.json",
              download_url: "https://example.test/doc_test_002.json",
            },
          ],
        ],
        [
          "https://example.test/doc_test_001.json",
          { id: "doc_test_001", title: "One", content: "First document." },
        ],
        [
          "https://example.test/doc_test_002.json",
          { id: "doc_test_002", title: "Two", content: "Second document." },
        ],
      ]);
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        requestedUrls.push(request.url);
        const responseBody = responses.get(request.url);
        assert(responseBody !== undefined);
        return Response.json(responseBody);
      };
      const semaphore = await runPromise(makeSemaphore(1));
      const load = ensureAllBankingDocuments(semaphore).pipe(
        provide(FetchHttpClient.layer)
      );
      await runPromise(load);
      await runPromise(load);
      expect(getAllBankingDocuments().map((document) => document.id)).toEqual([
        "doc_test_001",
        "doc_test_002",
      ]);
      expect(requestedUrls).toHaveLength(3);
    });
    it("normalizes a partially cached corpus to listing order", async () => {
      seedBankingDocumentsCache([
        { id: "doc_test_002", title: "Two", content: "Second document." },
      ]);
      const listingUrl =
        "https://api.github.com/repos/sierra-research/tau2-bench/contents/data/tau2/domains/banking_knowledge/documents?ref=fc0055dc4e0a316c3f83133267fbd6faaa770992";
      const responses = new Map<string, unknown>([
        [
          listingUrl,
          [
            {
              name: "doc_test_001.json",
              download_url: "https://example.test/doc_test_001.json",
            },
            {
              name: "doc_test_002.json",
              download_url: "https://example.test/doc_test_002.json",
            },
          ],
        ],
        [
          "https://example.test/doc_test_001.json",
          { id: "doc_test_001", title: "One", content: "First document." },
        ],
      ]);
      global.fetch = async (input, init) => {
        const request = new Request(input, init);
        const responseBody = responses.get(request.url);
        assert(responseBody !== undefined);
        return Response.json(responseBody);
      };
      const semaphore = await runPromise(makeSemaphore(1));
      await runPromise(
        ensureAllBankingDocuments(semaphore).pipe(
          provide(FetchHttpClient.layer)
        )
      );
      expect(getAllBankingDocuments().map((document) => document.id)).toEqual([
        "doc_test_001",
        "doc_test_002",
      ]);
    });
  });
});
