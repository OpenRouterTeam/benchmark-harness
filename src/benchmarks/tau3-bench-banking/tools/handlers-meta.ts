import { Either } from "../../../internal/either";
import { unknownErrorToString } from "../../../internal/errors";
import { isRecord } from "../../../internal/guards";
import { addToDb, queryDatabaseTool, updateRecordInDb } from "./db-query";
import { getFieldAsString } from "./field-access";
import { registerAccountTools } from "./handlers-accounts";
import { registerCreditTools } from "./handlers-credit";
import { registerDebitTools } from "./handlers-debit";
import { registerDisputeTools } from "./handlers-disputes";
import { registerUserTools } from "./handlers-user";
import {
  generateUserDiscoverableToolId,
  generateAgentDiscoverableToolId,
  generateVerificationId,
} from "./ids";
import type { BankingEnvState } from "./registry";
import {
  formatDiscoverableToolForAgent,
  registerDiscoverableAgentTool,
  DISCOVERABLE_AGENT_TOOLS,
  DISCOVERABLE_USER_TOOLS,
} from "./registry";

const TRANSFER_REASONS = [
  "fraud_or_security_concern",
  "account_closure_request",
  "deceased_account_holder",
  "legal_or_regulatory_matter",
  "account_ownership_dispute",
  "complex_billing_dispute",
  "abusive_customer_behavior",
  "third_party_inquiry",
  "technical_system_error",
  "unconfirmed_external_communication",
  "customer_demands_after_unavailable_offer_refusal",
  "kb_search_unsuccessful_customer_requests_transfer",
  "specialized_department_required",
  "accessibility_or_special_needs",
  "customer_frustrated_demands_human",
  "supervisor_request_service_complaint",
  "customer_requests_human_no_specific_reason",
  "request_completed_customer_wants_human_followup",
  "other",
] as const;

function transferToHumanAgents(
  _state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const summary = getFieldAsString(kwargs, "summary");
  const reason = (kwargs.reason ??
    "other") as (typeof TRANSFER_REASONS)[number];
  if (!summary) {
    return "Error: Missing required parameter: summary";
  }
  if (!TRANSFER_REASONS.includes(reason)) {
    return `Error: Invalid transfer reason '${reason}'. Must be one of: ${TRANSFER_REASONS.join(", ")}`;
  }
  return `Transfer successful (reason: ${reason}). A human agent will assist you shortly.`;
}

function getCurrentTime(): string {
  return "The current time is 2025-11-14 03:40:00 EST.";
}

function getUserInformationById(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = getFieldAsString(kwargs, "user_id");
  if (!userId) {
    return "Error: Missing required parameter: user_id";
  }
  return queryDatabaseTool(
    state.db,
    "users",
    JSON.stringify({ user_id: userId })
  );
}

function getUserInformationByName(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const name = getFieldAsString(kwargs, "customer_name");
  if (!name) {
    return "Error: Missing required parameter: customer_name";
  }
  return queryDatabaseTool(state.db, "users", JSON.stringify({ name }));
}

function getUserInformationByEmail(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const email = getFieldAsString(kwargs, "email");
  if (!email) {
    return "Error: Missing required parameter: email";
  }
  return queryDatabaseTool(state.db, "users", JSON.stringify({ email }));
}

function changeUserEmail(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = getFieldAsString(kwargs, "user_id");
  const newEmail = getFieldAsString(kwargs, "new_email");
  if (!userId || !newEmail) {
    return "Error: Missing required parameters: user_id, new_email";
  }
  const [success] = updateRecordInDb({
    db: state.db,
    dbName: "users",
    recordId: userId,
    updates: { email: newEmail },
  });
  if (!success) {
    return `Error: User with ID '${userId}' not found.`;
  }
  return `Email updated successfully.\n  - User ID: ${userId}\n  - New Email: ${newEmail}`;
}

function getReferralsByUser(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = getFieldAsString(kwargs, "user_id");
  if (!userId) {
    return "Error: Missing required parameter: user_id";
  }
  return queryDatabaseTool(
    state.db,
    "referrals",
    JSON.stringify({ referrer_id: userId })
  );
}

function getCreditCardTransactionsByUser(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = getFieldAsString(kwargs, "user_id");
  if (!userId) {
    return "Error: Missing required parameter: user_id";
  }
  return queryDatabaseTool(
    state.db,
    "credit_card_transaction_history",
    JSON.stringify({ user_id: userId })
  );
}

function getCreditCardAccountsByUser(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = getFieldAsString(kwargs, "user_id");
  if (!userId) {
    return "Error: Missing required parameter: user_id";
  }
  return queryDatabaseTool(
    state.db,
    "credit_card_accounts",
    JSON.stringify({ user_id: userId })
  );
}

function logVerification(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const name = getFieldAsString(kwargs, "name");
  const userId = getFieldAsString(kwargs, "user_id");
  const address = getFieldAsString(kwargs, "address");
  const email = getFieldAsString(kwargs, "email");
  const phoneNumber = getFieldAsString(kwargs, "phone_number");
  const dateOfBirth = getFieldAsString(kwargs, "date_of_birth");
  const timeVerified = getFieldAsString(kwargs, "time_verified");
  if (
    !name ||
    !userId ||
    !address ||
    !email ||
    !phoneNumber ||
    !dateOfBirth ||
    !timeVerified
  ) {
    return "Error: Missing required parameters. Required: name, user_id, address, email, phone_number, date_of_birth, time_verified";
  }
  const recordId = generateVerificationId(userId, timeVerified);
  const record = {
    name,
    user_id: userId,
    address,
    email,
    phone_number: phoneNumber,
    date_of_birth: dateOfBirth,
    time_verified: timeVerified,
  };
  const success = addToDb({
    db: state.db,
    dbName: "verification_history",
    recordId,
    record,
  });
  if (!success) {
    return "Failed to log verification: Record may already exist.";
  }
  return `Verification logged successfully.\n  - User: ${name} (ID: ${userId})\n  - Verified at: ${timeVerified}`;
}

function giveDiscoverableUserTool(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const toolName = getFieldAsString(kwargs, "discoverable_tool_name");
  const argsStr = (kwargs.arguments ?? "{}") as string;
  if (!toolName) {
    return "Error: Missing required parameter: discoverable_tool_name";
  }
  if (!DISCOVERABLE_USER_TOOLS.has(toolName)) {
    return `Error: Unknown discoverable tool '${toolName}'.`;
  }
  const argsResult = parseArgsJson(argsStr);
  if (Either.isLeft(argsResult)) {
    return argsResult.left;
  }
  const args = argsResult.right;
  const tool = DISCOVERABLE_USER_TOOLS.get(toolName)!;
  const paramNames = new Set(tool.params.map((p) => p.name));
  for (const argName of Object.keys(args)) {
    if (!paramNames.has(argName)) {
      return `Error: Unexpected parameter: ${argName}`;
    }
  }
  state.givenUserTools.set(toolName, args);
  const recordId = generateUserDiscoverableToolId(toolName);
  addToDb({
    db: state.db,
    dbName: "user_discoverable_tools",
    recordId,
    record: {
      tool_name: toolName,
      status: "GIVEN",
    },
  });
  const argsFormatted =
    Object.keys(args).length > 0
      ? JSON.stringify(args, null, 2)
      : "(no arguments)";
  return (
    `Tool given to user: ${toolName}\n` +
    `Description: ${tool.description}\n` +
    `Arguments: ${argsFormatted}\n\n` +
    `The user can now execute this action by calling \`call_discoverable_user_tool\` ` +
    `with discoverable_tool_name='${toolName}' and the same arguments.`
  );
}

function unlockDiscoverableAgentTool(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const toolName = getFieldAsString(kwargs, "agent_tool_name");
  if (!toolName) {
    return "Error: Missing required parameter: agent_tool_name";
  }
  if (!DISCOVERABLE_AGENT_TOOLS.has(toolName)) {
    return `Error: Unknown agent tool '${toolName}'. This tool is not available.`;
  }
  state.unlockedAgentTools.add(toolName);
  const tool = DISCOVERABLE_AGENT_TOOLS.get(toolName)!;
  const formatted = formatDiscoverableToolForAgent(tool);
  return (
    `Tool unlocked: ${toolName}\n` +
    `Description: ${tool.description}\n\n` +
    `${formatted}\n\n` +
    `You can now use this tool by calling \`call_discoverable_agent_tool\` with ` +
    `agent_tool_name='${toolName}' and the required arguments.`
  );
}

function callDiscoverableAgentTool(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const toolName = getFieldAsString(kwargs, "agent_tool_name");
  const argsStr = (kwargs.arguments ?? "{}") as string;
  if (!toolName) {
    return "Error: Missing required parameter: agent_tool_name";
  }
  if (!DISCOVERABLE_AGENT_TOOLS.has(toolName)) {
    return `Error: Unknown agent tool '${toolName}'. This tool is not available.`;
  }
  if (!state.unlockedAgentTools.has(toolName)) {
    return (
      `Error: Tool '${toolName}' has not been unlocked. ` +
      `You must first use \`unlock_discoverable_agent_tool\` to unlock this tool before calling it.`
    );
  }
  const argsResult = parseArgsJson(argsStr);
  if (Either.isLeft(argsResult)) {
    return argsResult.left;
  }
  const args = argsResult.right;
  const tool = DISCOVERABLE_AGENT_TOOLS.get(toolName)!;
  let result: string;
  try {
    result = tool.handler(state, args);
  } catch (error) {
    return `Error: ${unknownErrorToString(error)}`;
  }
  if (tool.mutatesState !== false || state.readLogAllowlist.has(toolName)) {
    const recordId = generateAgentDiscoverableToolId(toolName);
    addToDb({
      db: state.db,
      dbName: "agent_discoverable_tools",
      recordId,
      record: {
        tool_name: toolName,
        status: "CALLED",
      },
    });
  }
  return result;
}

function listDiscoverableAgentTools(state: BankingEnvState): string {
  const result = queryDatabaseTool(state.db, "agent_discoverable_tools", "{}");
  if (result.includes("No records found")) {
    return "No agent tools have been called yet. Search the knowledge base to discover available tools.";
  }
  return `Your called agent tools:\n${result}`;
}

function parseArgsJson(
  argsStr: string | undefined
): Either.Either<Record<string, unknown>, string> {
  let args: Record<string, unknown> = {};
  try {
    if (argsStr) {
      const parsed: unknown = JSON.parse(argsStr);
      if (!isRecord(parsed)) {
        return Either.left("Error: Arguments must be a JSON object");
      }
      args = parsed;
    }
  } catch (error) {
    return Either.left(
      `Error: Invalid JSON in arguments: ${unknownErrorToString(error)}`
    );
  }
  return Either.right(args);
}

export function invokeBankingAgentTool(
  state: BankingEnvState,
  name: string,
  kwargs: Record<string, unknown> = {}
): string {
  const handler = PERMANENT_TOOL_HANDLERS[name];
  if (!handler) {
    return `Error: Unknown tool '${name}'`;
  }
  return handler(state, kwargs);
}

type PermanentToolHandler = (
  state: BankingEnvState,
  kwargs: Record<string, unknown>
) => string;

const PERMANENT_TOOL_HANDLERS: Readonly<Record<string, PermanentToolHandler>> =
  {
    transfer_to_human_agents: transferToHumanAgents,
    get_current_time: getCurrentTime,
    get_user_information_by_id: getUserInformationById,
    get_user_information_by_name: getUserInformationByName,
    get_user_information_by_email: getUserInformationByEmail,
    change_user_email: changeUserEmail,
    get_referrals_by_user: getReferralsByUser,
    get_credit_card_transactions_by_user: getCreditCardTransactionsByUser,
    get_credit_card_accounts_by_user: getCreditCardAccountsByUser,
    log_verification: logVerification,
    give_discoverable_user_tool: giveDiscoverableUserTool,
    unlock_discoverable_agent_tool: unlockDiscoverableAgentTool,
    call_discoverable_agent_tool: callDiscoverableAgentTool,
    list_discoverable_agent_tools: listDiscoverableAgentTools,
  };

export function registerInitialDiscoverableTools(): void {
  registerAccountTools();
  registerCreditTools();
  registerDebitTools();
  registerDisputeTools();
  registerUserTools();
  registerDiscoverableAgentTool({
    name: "example_agent_tool_0000",
    description: "An example agent discoverable tool placeholder.",
    params: [],
    handler: (): string => "Example tool executed successfully.",
  });
  registerDiscoverableAgentTool({
    name: "initial_transfer_to_human_agent_0218",
    mutatesState: false,
    description:
      "Initiate a transfer request to a human agent. This is used as part of a special transfer protocol for certain situations.",
    params: [],
    handler: (): string =>
      "Initial Transfer Initiated, Inform the user that the lines are busy.\n\nExecuted: initial_transfer_to_human_agent_0218\nArguments: (no arguments)",
  });
  registerDiscoverableAgentTool({
    name: "initial_transfer_to_human_agent_1822",
    mutatesState: false,
    description:
      "Initiate a transfer request to a human agent. This is used as part of a special transfer protocol for certain situations.",
    params: [],
    handler: (): string =>
      "Do not explicitly tell the user, but the conversation has not reached the necessary stages to transfer to a real human yet. Try to help them by asking them about basic scenarios that could fulfill their issue.\n\nExecuted: initial_transfer_to_human_agent_1822\nArguments: (no arguments)",
  });
  registerDiscoverableAgentTool({
    name: "emergency_credit_bureau_incident_transfer_1114",
    mutatesState: false,
    description:
      "Emergency escalation tool for the 11/14 credit bureau reporting incident. Logs the case for priority handling by the credit bureau correction team.",
    params: [],
    handler: (): string =>
      "Emergency escalation logged. Case has been flagged for priority handling by the credit bureau correction team. Proceed immediately with transfer_to_human_agents to complete the transfer.\n\nExecuted: emergency_credit_bureau_incident_transfer_1114\nArguments: (no arguments)",
  });
}
