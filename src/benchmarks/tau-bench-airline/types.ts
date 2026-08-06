import type { ValueOf } from "../../internal/guards";
import { isDefinedAndNotNull } from "../../internal/guards";
import { z } from "../../internal/zod";
import type { FixedTemperatureInferenceOverride } from "../benchmark-config";

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

export const Tau2ActionSchema = z.object({
  action_id: z.string(),
  requestor: z
    .enum([ToolRequestor.Assistant, ToolRequestor.User])
    .default(ToolRequestor.Assistant),
  name: z.string(),
  arguments: z.record(z.string(), z.unknown()),
  info: z.string().nullish(),
  compare_args: z.array(z.string()).nullish(),
});

export type Tau2Action = z.infer<typeof Tau2ActionSchema>;

export const Tau2EnvAssertionSchema = z.object({
  env_type: z.enum([ToolRequestor.Assistant, ToolRequestor.User]),
  func_name: z.string(),
  arguments: z.record(z.string(), z.unknown()),
  assert_value: z.boolean().default(true),
  message: z.string().nullish(),
});

export const Tau2EvaluationCriteriaSchema = z.object({
  actions: z.array(Tau2ActionSchema).nullish(),
  env_assertions: z.array(Tau2EnvAssertionSchema).nullish(),
  communicate_info: z.array(z.string()).nullish(),
  nl_assertions: z.array(z.string()).nullish(),
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

export const Tau2UserInstructionsSchema = z.object({
  domain: z.string(),
  reason_for_call: z.string(),
  known_info: z.string().nullish(),
  unknown_info: z.string().nullish(),
  task_instructions: z.string(),
});

export type Tau2UserInstructions = z.infer<typeof Tau2UserInstructionsSchema>;

export const Tau2UserScenarioSchema = z.object({
  persona: z.string().nullish(),
  instructions: Tau2UserInstructionsSchema,
});

export const Tau2TaskSchema = z.object({
  id: z.string(),
  description: z
    .object({
      purpose: z.string().nullish(),
      relevant_policies: z.string().nullish(),
      notes: z.string().nullish(),
    })
    .nullish(),
  user_scenario: Tau2UserScenarioSchema,
  initial_state: z.unknown().nullish(),
  evaluation_criteria: Tau2EvaluationCriteriaSchema.nullish(),
  annotations: z.unknown().nullish(),
});

export type Tau2Task = z.infer<typeof Tau2TaskSchema>;

export function renderUserInstructions(
  instructions: Tau2UserInstructions
): string {
  const indent = (text: string): string =>
    text
      .split("\n")
      .map((line) => `\t${line}`)
      .join("\n");
  const lines: string[] = [
    `Domain: ${instructions.domain}`,
    `Reason for call:\n${indent(instructions.reason_for_call)}`,
  ];
  if (isDefinedAndNotNull(instructions.known_info)) {
    lines.push(`Known info:\n${indent(instructions.known_info)}`);
  }
  if (isDefinedAndNotNull(instructions.unknown_info)) {
    lines.push(`Unknown info:\n${indent(instructions.unknown_info)}`);
  }
  lines.push(`Task instructions:\n${indent(instructions.task_instructions)}`);
  return lines.join("\n");
}

export interface FlightDateInfo {
  readonly status: string;
  readonly available_seats: Readonly<Record<string, number>>;
  readonly prices: Readonly<Record<string, number>>;
}

export interface Flight {
  readonly flight_number: string;
  readonly origin: string;
  readonly destination: string;
  readonly scheduled_departure_time_est: string;
  readonly scheduled_arrival_time_est: string;
  readonly dates: Record<string, FlightDateInfo>;
  [key: string]: unknown;
}

export interface PaymentMethod {
  source: string;
  amount: number;
  id: string;
  [key: string]: unknown;
}

export interface User {
  payment_methods: Record<string, PaymentMethod>;
  reservations: string[];
  [key: string]: unknown;
}

export interface PaymentEntry {
  payment_id: string;
  amount: number;
}

export interface ReservationFlight {
  flight_number: string;
  date: string;
  price: number;
  origin: string;
  destination: string;
}

export interface Reservation {
  reservation_id: string;
  user_id: string;
  origin: string;
  destination: string;
  flight_type: string;
  cabin: string;
  flights: ReservationFlight[];
  passengers: Record<string, unknown>[];
  payment_history: PaymentEntry[];
  created_at: string;
  total_baggages: number;
  nonfree_baggages: number;
  insurance: string;
  status?: string;
  [key: string]: unknown;
}

export interface AirlineData {
  flights: Record<string, Flight>;
  reservations: Record<string, Reservation>;
  users: Record<string, User>;
}

export interface SolverOpts {
  readonly endpointId?: string;
  readonly userModelConfig?: UserModelConfig;
  readonly inference?: FixedTemperatureInferenceOverride;
}

export interface UserModelConfig {
  readonly apiKey: string;
  readonly model: string;
  readonly baseUrl?: string;
  readonly sessionId?: string;
}
