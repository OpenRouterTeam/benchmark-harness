import type { ValueOf } from "../../internal/guards";
import { z } from "../../internal/zod";

export const OutputFormat = {
  Json: "json",
  Yaml: "yaml",
} as const;

export type OutputFormat = ValueOf<typeof OutputFormat>;

export interface JsonSchemaNode {
  readonly type?: string | readonly string[];
  readonly properties?: Readonly<Record<string, JsonSchemaNode>>;
  readonly items?: JsonSchemaNode;
  readonly required?: readonly string[];
  readonly enum?: readonly unknown[];
  readonly minimum?: number;
  readonly maximum?: number;
  readonly minItems?: number;
  readonly maxItems?: number;
}

export const JsonSchemaNodeSchema: z.ZodType<JsonSchemaNode> = z.lazy(() =>
  z
    .object({
      type: z.union([z.string(), z.array(z.string())]).optional(),
      properties: z.record(z.string(), JsonSchemaNodeSchema).optional(),
      items: JsonSchemaNodeSchema.optional(),
      required: z.array(z.string()).optional(),
      enum: z.array(z.unknown()).optional(),
      minimum: z.number().optional(),
      maximum: z.number().optional(),
      minItems: z.number().optional(),
      maxItems: z.number().optional(),
    })
    .passthrough()
);

export const TopLevelCountSchema = z.union([
  z.number().int(),
  z.tuple([z.number(), z.number()]),
]);

export type TopLevelCount = z.infer<typeof TopLevelCountSchema> | null;

export const IfStructRequirementsSchema = z.object({
  jsonSchema: JsonSchemaNodeSchema,
  topLevelCount: z.union([
    z.number().int(),
    z.tuple([z.number(), z.number()]),
    z.null(),
  ]),
  topLevelKey: z.string().nullable(),
  requireWrapperKey: z.boolean(),
  requireCodeBlock: z.boolean(),
  requireNoCommentary: z.boolean(),
  outputFormat: z.enum([OutputFormat.Json, OutputFormat.Yaml]),
});

export type IfStructRequirements = z.infer<typeof IfStructRequirementsSchema>;
