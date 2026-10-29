import { describe, expect, it } from "bun:test";

import { assertSuccess } from "../../internal/testing";
import { parseSchema } from "../../internal/zod";
import { BankingRetrievalConfigSchema } from "./retrieval-config";
describe("BankingRetrievalConfigSchema", () => {
  it("defaults to bm25_grep for AA parity", () => {
    const result = parseSchema(BankingRetrievalConfigSchema, undefined);
    assertSuccess(result);
    expect(result.success).toBe("bm25_grep");
  });
  it("accepts bm25_grep", () => {
    const result = parseSchema(BankingRetrievalConfigSchema, "bm25_grep");
    assertSuccess(result);
    expect(result.success).toBe("bm25_grep");
  });
});
