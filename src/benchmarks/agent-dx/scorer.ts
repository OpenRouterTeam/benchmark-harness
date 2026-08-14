import type { ScorerService } from "../../harness/scorer";
import { makeRewardScorer } from "../harbor/reward";
import { readAgentDxMeta } from "./dataset";

export const agentDxScorer: ScorerService = makeRewardScorer((metadata) => {
  const meta = readAgentDxMeta(metadata);
  if (meta === undefined) {
    return undefined;
  }
  return {
    ...(meta.reward !== undefined && { reward: meta.reward }),
    ...(meta.testOutput !== undefined && { verifierOutput: meta.testOutput }),
  };
});
