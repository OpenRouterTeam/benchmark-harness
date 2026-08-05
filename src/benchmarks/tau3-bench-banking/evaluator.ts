import { isDefinedAndNotNull } from "../../internal/guards";
import { compareActionWithToolCall } from "./action-match";
import { applyInitialState, dbHash, loadBankingData } from "./environment";
import { TerminationReason } from "./solver";
import {
  invokeBankingAgentTool,
  registerInitialDiscoverableTools,
} from "./tools/handlers-meta";
import { invokeBankingUserTool } from "./tools/handlers-user";
import { makeBankingEnvState } from "./tools/registry";
import type { BankingData, Tau3Action, Tau3Task } from "./types";
import {
  DEFAULT_REWARD_BASIS,
  deriveReadLogAllowlist,
  RewardType,
} from "./types";

export interface PredictedToolCall {
  readonly name: string;
  readonly arguments: Readonly<Record<string, unknown>>;
  readonly requestor: "assistant" | "user";
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
  readonly task: Tau3Task | undefined;
  readonly agentData: BankingData;
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
    terminationReason !== TerminationReason.AgentStop
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
  const unsupportedBases = basis.filter(
    (rewardBasis) => !isSupportedRewardBasis(rewardBasis)
  );
  if (unsupportedBases.length > 0) {
    throw new Error(
      `Banking task ${task.id} uses unsupported reward basis: ${unsupportedBases.join(", ")}`
    );
  }
  const dbReward = computeDbReward(criteria.actions, agentData, task);
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

function isSupportedRewardBasis(basis: RewardType): boolean {
  switch (basis) {
    case RewardType.Db: {
      return true;
    }
    case RewardType.Action: {
      return true;
    }
    case RewardType.Communicate: {
      return true;
    }
    case RewardType.EnvAssertion: {
      return false;
    }
    case RewardType.NlAssertion: {
      return false;
    }
    default: {
      return basis satisfies never;
    }
  }
}

function computeDbReward(
  goldenActions: readonly Tau3Action[] | null | undefined,
  agentData: BankingData,
  task: Tau3Task
): number {
  if (!isDefinedAndNotNull(goldenActions)) {
    return 1;
  }
  const goldData = loadBankingData();
  applyInitialState(goldData, task);
  registerInitialDiscoverableTools();
  const envState = makeBankingEnvState(goldData, deriveReadLogAllowlist(task));
  for (const action of goldenActions) {
    if (action.requestor === "user") {
      invokeBankingUserTool(envState, action.name, action.arguments);
    } else {
      invokeBankingAgentTool(envState, action.name, action.arguments);
    }
  }
  return dbHash(goldData) === dbHash(agentData) ? 1 : 0;
}

function computeActionReward(
  goldenActions: readonly Tau3Action[],
  toolCalls: readonly PredictedToolCall[]
): number {
  if (goldenActions.length === 0) {
    return 1;
  }
  const allMet = goldenActions.every((action) =>
    toolCalls.some(
      (tc) =>
        tc.requestor === action.requestor &&
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
  const concatenated = assistantTexts.join(" ");
  const allMet = communicateInfo.every((info) => concatenated.includes(info));
  return allMet ? 1 : 0;
}
