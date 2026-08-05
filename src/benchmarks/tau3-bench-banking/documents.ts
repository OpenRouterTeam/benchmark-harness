import type { HttpClientError } from "@effect/platform";
import { HttpClient } from "@effect/platform";
import { TaggedError } from "effect/Data";
import type { Effect, Semaphore } from "effect/Effect";
import { fail, forEach, gen, succeed } from "effect/Effect";

import { Either } from "../../internal/either";
import { parseSchema, z } from "../../internal/zod";
import {
  BANKING_SOURCE_BASE_URL,
  BANKING_SOURCE_REVISION,
} from "./environment";

export interface BankingDocument {
  readonly id: string;
  readonly title: string;
  readonly content: string;
}

interface DocumentListingEntry {
  readonly name: string;
  readonly download_url: string;
}

const BankingDocumentSchema: z.ZodType<BankingDocument> = z.object({
  id: z.string(),
  title: z.string(),
  content: z.string(),
});

const DocumentListingEntrySchema: z.ZodType<DocumentListingEntry> = z.object({
  name: z.string(),
  download_url: z.string(),
});

const DocumentListingSchema = z.array(DocumentListingEntrySchema);

const BANKING_DOCUMENTS_CACHE = new Map<string, BankingDocument>();

let isFullCorpusCached = false;

const DOCUMENT_BASE_URL = `${BANKING_SOURCE_BASE_URL}/documents`;

const DOCUMENT_LISTING_URL = `https://api.github.com/repos/sierra-research/tau2-bench/contents/data/tau2/domains/banking_knowledge/documents?ref=${BANKING_SOURCE_REVISION}`;

class DocumentFetchError extends TaggedError("DocumentFetchError")<{
  readonly message: string;
}> {}

export function ensureBankingDocuments(
  docIds: readonly string[],
  fetchLock: Semaphore
): Effect<
  void,
  DocumentFetchError | HttpClientError.HttpClientError,
  HttpClient.HttpClient
> {
  if (docIds.every((id) => BANKING_DOCUMENTS_CACHE.has(id))) {
    return succeed(undefined);
  }
  return fetchLock.withPermits(1)(
    gen(function* () {
      const client = yield* HttpClient.HttpClient;
      const missingIds = docIds.filter(
        (id) => !BANKING_DOCUMENTS_CACHE.has(id)
      );
      const fetchedDocuments = yield* forEach(
        missingIds,
        (id) => {
          const encodedId = encodeURIComponent(id);
          const url = `${DOCUMENT_BASE_URL}/${encodedId}.json`;
          return fetchBankingDocument({ client, id, url });
        },
        { concurrency: 16 }
      );
      for (const document of fetchedDocuments) {
        BANKING_DOCUMENTS_CACHE.set(document.id, document);
      }
    })
  );
}

export function ensureAllBankingDocuments(
  fetchLock: Semaphore
): Effect<
  void,
  DocumentFetchError | HttpClientError.HttpClientError,
  HttpClient.HttpClient
> {
  if (isFullCorpusCached) {
    return succeed(undefined);
  }
  return fetchLock.withPermits(1)(
    gen(function* () {
      if (isFullCorpusCached) {
        return;
      }
      const client = yield* HttpClient.HttpClient;
      const response = yield* client.get(DOCUMENT_LISTING_URL, {
        headers: {
          Accept: "application/vnd.github+json",
          "User-Agent": "openrouter-bench-harness",
        },
      });
      if (response.status < 200 || response.status >= 300) {
        return yield* fail(
          new DocumentFetchError({
            message: `HTTP ${response.status} when fetching banking document listing`,
          })
        );
      }
      const listingJson: unknown = yield* response.json;
      const listingResult = parseSchema(DocumentListingSchema, listingJson);
      if (Either.isLeft(listingResult)) {
        return yield* fail(
          new DocumentFetchError({
            message: `Invalid banking document listing: ${listingResult.left.message}`,
          })
        );
      }
      const missingEntries = listingResult.right.filter((entry) => {
        const id = entry.name.replace(/\.json$/u, "");
        return entry.name.endsWith(".json") && !BANKING_DOCUMENTS_CACHE.has(id);
      });
      const fetchedDocuments = yield* forEach(
        missingEntries,
        (entry) => {
          const id = entry.name.replace(/\.json$/u, "");
          return fetchBankingDocument({ client, id, url: entry.download_url });
        },
        { concurrency: 16 }
      );
      for (const document of fetchedDocuments) {
        BANKING_DOCUMENTS_CACHE.set(document.id, document);
      }
      const orderedDocuments: BankingDocument[] = [];
      for (const entry of listingResult.right) {
        if (!entry.name.endsWith(".json")) {
          continue;
        }
        const id = entry.name.replace(/\.json$/u, "");
        const document = BANKING_DOCUMENTS_CACHE.get(id);
        if (document === undefined) {
          return yield* fail(
            new DocumentFetchError({
              message: `Banking document listing entry ${entry.name} did not load`,
            })
          );
        }
        orderedDocuments.push(document);
      }
      BANKING_DOCUMENTS_CACHE.clear();
      for (const document of orderedDocuments) {
        BANKING_DOCUMENTS_CACHE.set(document.id, document);
      }
      isFullCorpusCached = true;
    })
  );
}

export function getBankingDocument(idOrTitle: string): BankingDocument {
  const byId = BANKING_DOCUMENTS_CACHE.get(idOrTitle);
  if (byId) {
    return byId;
  }
  for (const doc of BANKING_DOCUMENTS_CACHE.values()) {
    if (doc.title === idOrTitle) {
      return doc;
    }
  }
  throw new Error(
    `Banking document not found: ${idOrTitle} (not in cache by id or title)`
  );
}

export function getAllBankingDocuments(): readonly BankingDocument[] {
  return [...BANKING_DOCUMENTS_CACHE.values()];
}

export function seedBankingDocumentsCache(
  docs: readonly BankingDocument[]
): void {
  BANKING_DOCUMENTS_CACHE.clear();
  isFullCorpusCached = false;
  for (const doc of docs) {
    BANKING_DOCUMENTS_CACHE.set(doc.id, doc);
  }
}

function fetchBankingDocument({
  client,
  id,
  url,
}: {
  readonly client: HttpClient.HttpClient;
  readonly id: string;
  readonly url: string;
}): Effect<
  BankingDocument,
  DocumentFetchError | HttpClientError.HttpClientError
> {
  return gen(function* () {
    const response = yield* client.get(url);
    if (response.status < 200 || response.status >= 300) {
      return yield* fail(
        new DocumentFetchError({
          message: `HTTP ${response.status} when fetching document ${id}`,
        })
      );
    }
    const json: unknown = yield* response.json;
    const parseResult = parseSchema(BankingDocumentSchema, json);
    if (Either.isLeft(parseResult)) {
      return yield* fail(
        new DocumentFetchError({
          message: `Invalid document schema for ${id}: ${parseResult.left.message}`,
        })
      );
    }
    return parseResult.right;
  });
}
