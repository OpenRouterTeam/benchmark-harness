import type { ValueOf } from "../../internal/guards";
import { z, zInt } from "../../internal/zod";

export const CHESS_MOVE_LADDER_ID = "chess_move_ladder" as const;

export const CRITERIA_LEVELS = [
  "board",
  "instruction",
  "heuristics",
  "features",
  "motif",
] as const;

export type CriteriaLevel = ValueOf<typeof CRITERIA_LEVELS>;

export const CRITERION_IDS = [
  "material",
  "hanging",
  "forcing",
  "king_safety",
  "threats",
  "mobility",
] as const;

export type CriterionId = ValueOf<typeof CRITERION_IDS>;

export const HEURISTICS_VARIANTS = ["correct", "inverted"] as const;

export type HeuristicsVariant = ValueOf<typeof HEURISTICS_VARIANTS>;

export const BOARD_FORMATS = ["fen", "ascii", "both"] as const;

export type BoardFormat = ValueOf<typeof BOARD_FORMATS>;

export const MODEL_INTERFACES = ["chat", "systemone"] as const;

export type ModelInterface = ValueOf<typeof MODEL_INTERFACES>;

export const DEFAULT_CANDIDATE_COUNT = 4;

export const ChessMoveLadderOptionsSchema = z.object({
  criteriaLevel: z.enum(CRITERIA_LEVELS).default("instruction"),
  heuristicsVariant: z.enum(HEURISTICS_VARIANTS).default("correct"),
  ablateCriterion: z.enum(CRITERION_IDS).optional(),
  boardFormat: z.enum(BOARD_FORMATS).default("both"),
  candidateCount: zInt().min(2).max(8).default(DEFAULT_CANDIDATE_COUNT),
  interface: z.enum(MODEL_INTERFACES).optional(),
});

export type ChessMoveLadderOptions = z.infer<
  typeof ChessMoveLadderOptionsSchema
>;

export interface ChessCondition {
  readonly criteriaLevel: CriteriaLevel;
  readonly heuristicsVariant: HeuristicsVariant;
  readonly ablateCriterion?: CriterionId;
  readonly boardFormat: BoardFormat;
}

export const ChessConditionSchema = z.object({
  criteriaLevel: z.enum(CRITERIA_LEVELS),
  heuristicsVariant: z.enum(HEURISTICS_VARIANTS),
  ablateCriterion: z.enum(CRITERION_IDS).optional(),
  boardFormat: z.enum(BOARD_FORMATS),
});

export function conditionFromOptions(
  options: ChessMoveLadderOptions
): ChessCondition {
  return {
    criteriaLevel: options.criteriaLevel,
    heuristicsVariant: options.heuristicsVariant,
    ...(options.ablateCriterion !== undefined && {
      ablateCriterion: options.ablateCriterion,
    }),
    boardFormat: options.boardFormat,
  };
}

const SYSTEMONE_MODEL_PATTERN = /^(typesafe\/)?jev(-|$)/u;

export function resolveModelInterface(
  model: string,
  explicit: ModelInterface | undefined
): ModelInterface {
  if (explicit !== undefined) {
    return explicit;
  }
  return SYSTEMONE_MODEL_PATTERN.test(model) ? "systemone" : "chat";
}
