import type { ScorerService } from "../../harness/scorer";
import { makeRewardScorer } from "../harbor/reward";
import { readSweAtlasMeta } from "./dataset";

export const sweAtlasScorer: ScorerService = makeRewardScorer(readSweAtlasMeta);
