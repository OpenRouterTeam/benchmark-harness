import { z } from "../internal/zod";

export const ReasoningDetailsSchema = z.array(z.unknown()).readonly();

export type ReasoningDetails = z.infer<typeof ReasoningDetailsSchema>;

export function hasReasoningDetails(value: unknown): value is ReasoningDetails {
  return Array.isArray(value) && value.length > 0;
}
