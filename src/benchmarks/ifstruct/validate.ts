import { Either } from "../../internal/either";
import { definedValues } from "../../internal/guards";
import {
  checkForCommentary,
  checkForCommentaryYaml,
  checkUsesCodeBlock,
  removeThinkingTags,
} from "./extract-common";
import { extractJson } from "./extract-json";
import { extractYaml } from "./extract-yaml";
import type { IfStructRequirements } from "./schema";
import { OutputFormat } from "./schema";
import type { FieldCheck } from "./schema-check";
import {
  checkTopLevelCount,
  checkTopLevelStructure,
  computeExpectedChecks,
  validateAgainstJsonSchema,
} from "./schema-check";

export interface ValidationDetails {
  readonly outputFormat: string;
  readonly usesCodeBlock: boolean;
  readonly codeBlockType: string | null;
  readonly jsonValid?: boolean;
  readonly yamlValid?: boolean;
  readonly noCommentary?: boolean;
  readonly wasWrapped?: boolean;
  readonly schemaValid?: boolean;
  readonly schemaFieldsTotal?: number;
  readonly schemaFieldsPassed?: number;
  readonly schemaMatchRatio?: number;
}

export interface ValidationResult {
  readonly passed: boolean;
  readonly score: number;
  readonly errors: readonly string[];
  readonly details: ValidationDetails;
}

export function validateResponse(
  rawResponse: string,
  requirements: IfStructRequirements
): ValidationResult {
  const response = removeThinkingTags(rawResponse);
  const isYaml = requirements.outputFormat === OutputFormat.Yaml;
  const errors: string[] = [];
  const { usesCodeBlock, codeBlockType } = checkUsesCodeBlock(response);
  if (requirements.requireCodeBlock && !usesCodeBlock) {
    errors.push("Response must use a code block but none was found");
  }
  const extracted = isYaml ? extractYaml(response) : extractJson(response);
  const extractValid = Either.isRight(extracted);
  if (Either.isLeft(extracted)) {
    return {
      passed: false,
      score: 0,
      errors: [...errors, extracted.left],
      details: {
        outputFormat: requirements.outputFormat,
        usesCodeBlock,
        codeBlockType,
        ...(isYaml ? { yamlValid: false } : { jsonValid: false }),
      },
    };
  }
  const checkCommentary = isYaml ? checkForCommentaryYaml : checkForCommentary;
  const commentary = requirements.requireNoCommentary
    ? checkCommentary(response)
    : null;
  if (
    commentary !== null &&
    commentary.hasCommentary &&
    commentary.description !== null
  ) {
    errors.push(commentary.description);
  }
  const structure = checkTopLevelStructure(
    extracted.right,
    requirements.topLevelKey,
    requirements.requireWrapperKey
  );
  if (structure.error !== undefined) {
    errors.push(structure.error);
  }
  const fieldChecks = validateAgainstJsonSchema(
    structure.data,
    requirements.jsonSchema
  );
  const schemaErrors = fieldChecks
    .filter(
      (
        check
      ): check is FieldCheck & {
        error: string;
      } => !check.passed && check.error !== undefined
    )
    .map((check) => check.error);
  errors.push(...schemaErrors);
  const schemaValid = schemaErrors.length === 0;
  const countError = checkTopLevelCount(
    structure.data,
    requirements.topLevelCount
  );
  if (countError !== null) {
    errors.push(countError);
  }
  const passedChecks = fieldChecks.filter((check) => check.passed).length;
  const totalChecks = Math.max(
    computeExpectedChecks(requirements.jsonSchema, requirements.topLevelCount),
    fieldChecks.length
  );
  const passed = errors.length === 0;
  return {
    passed,
    score: passed ? 1 : 0,
    errors,
    details: {
      outputFormat: requirements.outputFormat,
      usesCodeBlock,
      codeBlockType,
      ...(isYaml ? { yamlValid: extractValid } : { jsonValid: extractValid }),
      ...definedValues({
        noCommentary:
          commentary !== null ? !commentary.hasCommentary : undefined,
      }),
      wasWrapped: structure.wasWrapped,
      schemaValid,
      schemaFieldsTotal: totalChecks,
      schemaFieldsPassed: passedChecks,
      schemaMatchRatio: totalChecks > 0 ? passedChecks / totalChecks : 0,
    },
  };
}
