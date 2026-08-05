import type { ReasoningEffort } from "../../harness/constants";
import type { ValueOf } from "../../internal/guards";
import { z } from "../../internal/zod";
import type { InferenceOverride } from "../benchmark-config";
import type { BankingRetrievalConfig } from "./retrieval-config";

export const RewardType = {
  Db: "DB",
  EnvAssertion: "ENV_ASSERTION",
  NlAssertion: "NL_ASSERTION",
  Action: "ACTION",
  Communicate: "COMMUNICATE",
} as const;

export type RewardType = ValueOf<typeof RewardType>;

export const DEFAULT_REWARD_BASIS: readonly RewardType[] = [
  RewardType.Db,
  RewardType.Communicate,
];

export const ToolRequestor = {
  Assistant: "assistant",
  User: "user",
} as const;

export type ToolRequestor = ValueOf<typeof ToolRequestor>;

export const Tau3ActionSchema = z.object({
  action_id: z.string(),
  requestor: z
    .enum([ToolRequestor.Assistant, ToolRequestor.User])
    .default(ToolRequestor.Assistant),
  name: z.string(),
  arguments: z.record(z.string(), z.unknown()),
  compare_args: z.array(z.string()).nullish(),
  info: z.string().nullish(),
});

export type Tau3Action = z.infer<typeof Tau3ActionSchema>;

export const Tau3EvaluationCriteriaSchema = z.object({
  actions: z.array(Tau3ActionSchema).nullish(),
  communicate_info: z.array(z.string()).nullish(),
  reward_basis: z
    .array(
      z.enum([
        RewardType.Db,
        RewardType.EnvAssertion,
        RewardType.NlAssertion,
        RewardType.Action,
        RewardType.Communicate,
      ])
    )
    .nullish(),
});

export type Tau3EvaluationCriteria = z.infer<
  typeof Tau3EvaluationCriteriaSchema
>;

export const Tau3InitialStateSchema = z.object({
  initialization_data: z
    .object({
      agent_data: z.record(z.string(), z.unknown()).nullish(),
    })
    .nullable(),
  initialization_actions: z.unknown().nullish(),
  message_history: z.unknown().nullish(),
});

export type Tau3InitialState = z.infer<typeof Tau3InitialStateSchema>;

export const Tau3TaskSchema = z
  .object({
    id: z.string(),
    description: z
      .object({
        purpose: z.string().nullish(),
        relevant_policies: z.string().nullish(),
        notes: z.string().nullish(),
      })
      .nullish(),
    user_scenario: z.object({ instructions: z.string() }).passthrough(),
    initial_state: Tau3InitialStateSchema.nullish(),
    evaluation_criteria: Tau3EvaluationCriteriaSchema.nullish(),
    annotations: z.unknown().nullish(),
    user_tools: z.array(z.string()).nullish(),
    required_documents: z.array(z.string()).nullish(),
  })
  .passthrough();

export type Tau3Task = z.infer<typeof Tau3TaskSchema>;

export function deriveReadLogAllowlist(task: Tau3Task): ReadonlySet<string> {
  const allowlist = new Set<string>();
  for (const action of task.evaluation_criteria?.actions ?? []) {
    if (action.name !== "call_discoverable_agent_tool") {
      continue;
    }
    const toolName = action.arguments?.agent_tool_name;
    if (typeof toolName === "string" && toolName.length > 0) {
      allowlist.add(toolName);
    }
  }
  return allowlist;
}

export const BANKING_TABLES = [
  "users",
  "accounts",
  "debit_cards",
  "referrals",
  "credit_card_applications",
  "user_discoverable_tools",
  "user_discoverable_tool_calls",
  "verification_history",
  "credit_card_transaction_history",
  "cash_back_disputes",
  "bank_account_transaction_history",
  "credit_card_accounts",
  "agent_discoverable_tools",
  "task_config",
  "human_transfer_requests",
  "transaction_disputes",
  "credit_card_orders",
  "debit_card_orders",
  "credit_card_closure_reasons",
  "credit_card_account_flags",
  "credit_limit_increase_requests",
  "payment_history",
  "debit_card_disputes",
] as const;

export type BankingTableName = ValueOf<typeof BANKING_TABLES>;

export interface BankingTable {
  data: Record<string, Record<string, unknown>>;
  notes?: unknown;
}

export type BankingData = Record<BankingTableName, BankingTable>;

const BANKING_TABLE_NAME_SET: ReadonlySet<string> = new Set(BANKING_TABLES);

export function isBankingTableName(name: string): name is BankingTableName {
  return BANKING_TABLE_NAME_SET.has(name);
}

export function getBankingTable(
  db: BankingData,
  tableName: string
): BankingTable | undefined {
  if (!isBankingTableName(tableName)) {
    return undefined;
  }
  return db[tableName];
}

export interface UserModelConfig {
  readonly apiKey: string;
  readonly model: string;
  readonly baseUrl?: string;
  readonly sessionId?: string;
  readonly userReasoningEffort?: ReasoningEffort;
}

export interface SolverOpts {
  readonly endpointId?: string;
  readonly userModelConfig?: UserModelConfig;
  readonly retrievalConfig?: BankingRetrievalConfig;
  readonly inference?: InferenceOverride;
}
