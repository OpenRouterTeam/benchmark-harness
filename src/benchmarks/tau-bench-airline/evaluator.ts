import { isDefinedAndNotNull } from "../../internal/guards";
import { compareActionWithToolCall } from "./action-match";
import { dbHash, loadAirlineData } from "./environment";
import { TerminationReason } from "./solver";
import { invokeTool } from "./tools/handlers";
import type { AirlineData, Tau2Action, Tau2Task } from "./types";
import { DEFAULT_REWARD_BASIS, RewardType } from "./types";

export interface PredictedToolCall {
  readonly name: string;
  readonly arguments: Readonly<Record<string, unknown>>;
}

export interface RewardBreakdown {
  readonly db: number;
  readonly action: number;
  readonly communicate: number;
}

export interface EvaluationResult {
  readonly reward: number;
  readonly dbMatch: boolean;
  readonly actionMet: boolean;
  readonly communicateMet: boolean;
  readonly terminationReason: string;
  readonly note?: string;
  readonly breakdown: RewardBreakdown;
}

export interface EvaluateSimulationOpts {
  readonly task: Tau2Task | undefined;
  readonly agentData: AirlineData;
  readonly assistantTexts: readonly string[];
  readonly toolCalls?: readonly PredictedToolCall[];
  readonly terminationReason: string;
}

export function evaluateSimulation(
  opts: EvaluateSimulationOpts
): EvaluationResult {
  const {
    task,
    agentData,
    assistantTexts,
    toolCalls = [],
    terminationReason,
  } = opts;
  const zero = (note: string): EvaluationResult => ({
    reward: 0,
    dbMatch: false,
    actionMet: false,
    communicateMet: false,
    terminationReason,
    note,
    breakdown: { db: 0, action: 0, communicate: 0 },
  });
  if (
    terminationReason !== TerminationReason.UserStop &&
    terminationReason !== "AGENT_STOP"
  ) {
    return zero(`Simulation terminated prematurely: ${terminationReason}`);
  }
  const criteria = task?.evaluation_criteria;
  if (!task || criteria === null || criteria === undefined) {
    return {
      reward: 1,
      dbMatch: true,
      actionMet: true,
      communicateMet: true,
      terminationReason,
      note: "No evaluation criteria",
      breakdown: { db: 1, action: 1, communicate: 1 },
    };
  }
  const basis = criteria.reward_basis ?? DEFAULT_REWARD_BASIS;
  const dbReward = computeDbReward(
    criteria.actions,
    criteria.env_assertions,
    agentData
  );
  const actionReward = computeActionReward(criteria.actions ?? [], toolCalls);
  const communicateReward = computeCommunicateReward(
    criteria.communicate_info ?? [],
    assistantTexts
  );
  let reward = 1;
  if (basis.includes(RewardType.Db)) {
    reward *= dbReward;
  }
  if (basis.includes(RewardType.Action)) {
    reward *= actionReward;
  }
  if (basis.includes(RewardType.Communicate)) {
    reward *= communicateReward;
  }
  return {
    reward,
    dbMatch: dbReward >= 1,
    actionMet: actionReward >= 1,
    communicateMet: communicateReward >= 1,
    terminationReason,
    breakdown: {
      db: dbReward,
      action: actionReward,
      communicate: communicateReward,
    },
  };
}

function computeDbReward(
  goldenActions: readonly Tau2Action[] | null | undefined,
  envAssertions: readonly unknown[] | null | undefined,
  agentData: AirlineData
): number {
  if (
    !isDefinedAndNotNull(goldenActions) &&
    !isDefinedAndNotNull(envAssertions)
  ) {
    return 1;
  }
  const goldData = loadAirlineData();
  for (const action of goldenActions ?? []) {
    invokeTool(goldData, action.name, action.arguments);
  }
  return dbHash(goldData) === dbHash(agentData) ? 1 : 0;
}

function computeActionReward(
  goldenActions: readonly Tau2Action[],
  toolCalls: readonly PredictedToolCall[]
): number {
  if (goldenActions.length === 0) {
    return 1;
  }
  const allMet = goldenActions.every((action) =>
    toolCalls.some((tc) =>
      compareActionWithToolCall(action, tc.name, tc.arguments)
    )
  );
  return allMet ? 1 : 0;
}

function computeCommunicateReward(
  communicateInfo: readonly string[],
  assistantTexts: readonly string[]
): number {
  if (communicateInfo.length === 0) {
    return 1;
  }
  const haystacks = assistantTexts.map((t) =>
    t.toLowerCase().replaceAll(",", "")
  );
  const allMet = communicateInfo.every((info) => {
    const needle = info.toLowerCase();
    return haystacks.some((h) => h.includes(needle));
  });
  return allMet ? 1 : 0;
}
