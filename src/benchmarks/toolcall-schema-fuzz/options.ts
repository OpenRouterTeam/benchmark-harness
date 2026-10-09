import type { ValueOf } from "../../internal/guards";

export const ToolCallSchemaFuzzGenerator = {
  Realistic: "realistic",

  Regressions: "regressions",

  Sentinel: "sentinel",
} as const;
export type ToolCallSchemaFuzzGenerator = ValueOf<
  typeof ToolCallSchemaFuzzGenerator
>;
