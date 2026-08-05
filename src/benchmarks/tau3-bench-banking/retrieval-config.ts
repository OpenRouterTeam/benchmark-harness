import { z } from "../../internal/zod";

export const BANKING_RETRIEVAL_CONFIGS = [
  "required_docs",
  "bm25_grep",
] as const;

export const DEFAULT_BANKING_RETRIEVAL_CONFIG = "bm25_grep" as const;

export const BankingRetrievalConfigSchema = z
  .enum(BANKING_RETRIEVAL_CONFIGS)
  .default(DEFAULT_BANKING_RETRIEVAL_CONFIG);

export type BankingRetrievalConfig = z.infer<
  typeof BankingRetrievalConfigSchema
>;
