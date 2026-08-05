import { addToDb, queryDatabaseTool, updateRecordInDb } from "./db-query";
import { FROZEN_TODAY_STR as FROZEN_TODAY } from "./helpers";
import { generateDisputeId } from "./ids";
import type { BankingEnvState } from "./registry";
import { registerDiscoverableAgentTool } from "./registry";

function str(value: unknown): string {
  return String(value ?? "");
}

function num(value: unknown): number {
  if (typeof value === "number") {
    return value;
  }
  if (typeof value === "string") {
    const parsed = Number.parseFloat(value);
    return Number.isNaN(parsed) ? 0 : parsed;
  }
  return 0;
}

function bool(value: unknown): boolean {
  if (typeof value === "boolean") {
    return value;
  }
  if (typeof value === "string") {
    return value.toLowerCase() === "true";
  }
  return !!value;
}

function updateTransactionRewards(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const transactionId = str(kwargs.transaction_id);
  const newRewardsEarned = str(kwargs.new_rewards_earned);
  if (!transactionId || !newRewardsEarned) {
    return "Error: Missing required parameters.";
  }
  const [success] = updateRecordInDb({
    db: state.db,
    dbName: "credit_card_transaction_history",
    recordId: transactionId,
    updates: {
      rewards_earned: newRewardsEarned,
    },
  });
  if (!success) {
    return `Error: Transaction '${transactionId}' not found.`;
  }
  return (
    `Transaction rewards updated successfully.\n\n` +
    `Executed: update_transaction_rewards_3847\n` +
    `Arguments: ${JSON.stringify({ transaction_id: transactionId, new_rewards_earned: newRewardsEarned }, null, 2)}\n` +
    `Transaction updated:\n` +
    `  - Transaction ID: ${transactionId}\n` +
    `  - New Rewards: ${newRewardsEarned}`
  );
}

function fileCreditCardTransactionDispute(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const transactionId = str(kwargs.transaction_id);
  const cardAction = str(kwargs.card_action);
  const cardLast4Digits = str(kwargs.card_last_4_digits);
  const fullName = str(kwargs.full_name);
  const userId = str(kwargs.user_id);
  const phone = str(kwargs.phone);
  const email = str(kwargs.email);
  const address = str(kwargs.address);
  const contactedMerchant = bool(kwargs.contacted_merchant);
  const purchaseDate = str(kwargs.purchase_date);
  const issueNoticedDate = str(kwargs.issue_noticed_date);
  const disputeReason = str(kwargs.dispute_reason);
  const resolutionRequested = str(kwargs.resolution_requested);
  const eligibleForProvisionalCredit = bool(
    kwargs.eligible_for_provisional_credit
  );
  const partialRefundAmount = kwargs.partial_refund_amount
    ? num(kwargs.partial_refund_amount)
    : null;
  if (!transactionId || !userId) {
    return "Error: Missing required parameters.";
  }
  const validCardActions = ["keep_active", "cancel_and_reissue"];
  if (!validCardActions.includes(cardAction)) {
    return `Error: Invalid card_action. Must be one of: ${validCardActions}`;
  }
  const validReasons = [
    "unauthorized_fraudulent_charge",
    "duplicate_charge",
    "incorrect_amount",
    "goods_services_not_received",
    "goods_services_not_as_described",
    "canceled_subscription_still_charging",
    "refund_never_processed",
  ];
  if (!validReasons.includes(disputeReason)) {
    return `Error: Invalid dispute_reason. Must be one of: ${validReasons}`;
  }
  const validResolutions = ["full_refund", "partial_refund"];
  if (!validResolutions.includes(resolutionRequested)) {
    return `Error: Invalid resolution_requested. Must be one of: ${validResolutions}`;
  }
  if (
    resolutionRequested === "partial_refund" &&
    partialRefundAmount === null
  ) {
    return "Error: partial_refund_amount is required when resolution_requested is 'partial_refund'.";
  }
  const disputeId = generateDisputeId(userId, transactionId);
  const disputeRecord = {
    dispute_id: disputeId,
    transaction_id: transactionId,
    user_id: userId,
    card_action: cardAction,
    card_last_4_digits: cardLast4Digits,
    full_name: fullName,
    phone,
    email,
    address,
    contacted_merchant: contactedMerchant,
    purchase_date: purchaseDate,
    issue_noticed_date: issueNoticedDate,
    dispute_reason: disputeReason,
    resolution_requested: resolutionRequested,
    partial_refund_amount: partialRefundAmount,
    eligible_for_provisional_credit: eligibleForProvisionalCredit,
    provisional_credit_given: eligibleForProvisionalCredit,
    submitted_at: FROZEN_TODAY,
    status: "SUBMITTED",
  };
  const success = addToDb({
    db: state.db,
    dbName: "transaction_disputes",
    recordId: disputeId,
    record: disputeRecord,
  });
  if (!success) {
    return "Error: Dispute may have already been filed for this transaction.";
  }
  const resultParts = [
    "Credit card transaction dispute filed successfully. A case has been opened and will be reviewed within 10 business days.",
    "",
    "Executed: file_credit_card_transaction_dispute_4829",
    `Dispute ID: ${disputeId}`,
    `Transaction: ${transactionId}`,
    `Reason: ${disputeReason
      .replaceAll("_", " ")
      .split(" ")
      .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
      .join(" ")}`,
    `Resolution Requested: ${resolutionRequested
      .replaceAll("_", " ")
      .split(" ")
      .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
      .join(" ")}`,
  ];
  if (partialRefundAmount !== null) {
    resultParts.push(
      `Partial Refund Amount: $${partialRefundAmount.toFixed(2)}`
    );
  }
  if (eligibleForProvisionalCredit) {
    resultParts.push(
      "Provisional Credit: ELIGIBLE - Credit will be applied within 2 business days."
    );
  } else {
    resultParts.push("Provisional Credit: Not eligible at this time.");
  }
  return resultParts.join("\n");
}

function fileDebitCardTransactionDispute(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const transactionId = str(kwargs.transaction_id);
  const accountId = str(kwargs.account_id);
  const cardId = str(kwargs.card_id);
  const userId = str(kwargs.user_id);
  const disputeCategory = str(kwargs.dispute_category);
  const transactionDate = str(kwargs.transaction_date);
  const discoveryDate = str(kwargs.discovery_date);
  const disputedAmount = num(kwargs.disputed_amount);
  const transactionType = str(kwargs.transaction_type);
  const cardInPossession = bool(kwargs.card_in_possession);
  const pinCompromised = str(kwargs.pin_compromised);
  const contactedMerchant = bool(kwargs.contacted_merchant);
  const policeReportFiled = bool(kwargs.police_report_filed);
  const writtenStatementProvided = bool(kwargs.written_statement_provided);
  const provisionalCreditEligible = bool(kwargs.provisional_credit_eligible);
  const customerMaxLiabilityAmount = num(kwargs.customer_max_liability_amount);
  const cardAction = str(kwargs.card_action);
  if (
    !transactionId ||
    !accountId ||
    !cardId ||
    !userId ||
    !disputeCategory ||
    !transactionDate ||
    !discoveryDate ||
    !transactionType ||
    !pinCompromised ||
    !cardAction
  ) {
    return "Error: Missing required parameters.";
  }
  if (
    kwargs.customer_max_liability_amount === undefined ||
    kwargs.customer_max_liability_amount === null
  ) {
    return "Error: customer_max_liability_amount is required.";
  }
  if (
    kwargs.card_in_possession === undefined ||
    kwargs.card_in_possession === null ||
    kwargs.contacted_merchant === undefined ||
    kwargs.contacted_merchant === null ||
    kwargs.police_report_filed === undefined ||
    kwargs.police_report_filed === null ||
    kwargs.written_statement_provided === undefined ||
    kwargs.written_statement_provided === null
  ) {
    return "Error: card_in_possession, contacted_merchant, police_report_filed, and written_statement_provided are required boolean fields.";
  }
  const validCategories = [
    "unauthorized_transaction",
    "atm_cash_discrepancy",
    "atm_deposit_not_credited",
    "duplicate_charge",
    "incorrect_amount",
    "goods_services_not_received",
    "recurring_charge_after_cancellation",
    "card_present_fraud",
    "card_not_present_fraud",
  ];
  if (!validCategories.includes(disputeCategory)) {
    return `Error: Invalid dispute_category. Must be one of: ${validCategories}`;
  }
  const validTransactionTypes = [
    "pin_purchase",
    "signature_purchase",
    "online_purchase",
    "atm_withdrawal",
    "atm_deposit",
    "recurring_payment",
    "person_to_person",
  ];
  if (!validTransactionTypes.includes(transactionType)) {
    return `Error: Invalid transaction_type. Must be one of: ${validTransactionTypes}`;
  }
  const validPinStatuses = ["yes_shared", "yes_observed", "no", "unknown"];
  if (!validPinStatuses.includes(pinCompromised)) {
    return `Error: Invalid pin_compromised. Must be one of: ${validPinStatuses}`;
  }
  const validCardActions = [
    "keep_active",
    "freeze_pending_investigation",
    "close_and_reissue",
  ];
  if (!validCardActions.includes(cardAction)) {
    return `Error: Invalid card_action. Must be one of: ${validCardActions}`;
  }
  if (disputedAmount <= 0) {
    return "Error: disputed_amount must be a positive number.";
  }
  const disputeId = generateDisputeId(userId, transactionId);
  const isFraudCategory = [
    "unauthorized_transaction",
    "card_present_fraud",
    "card_not_present_fraud",
  ].includes(disputeCategory);
  const pinSharedVoluntarily = pinCompromised === "yes_shared";
  const disputeRecord = {
    dispute_id: disputeId,
    transaction_id: transactionId,
    account_id: accountId,
    card_id: cardId,
    user_id: userId,
    dispute_category: disputeCategory,
    transaction_date: transactionDate,
    discovery_date: discoveryDate,
    disputed_amount: disputedAmount,
    transaction_type: transactionType,
    card_in_possession: cardInPossession,
    pin_compromised: pinCompromised,
    contacted_merchant: contactedMerchant,
    police_report_filed: policeReportFiled,
    written_statement_provided: writtenStatementProvided,
    provisional_credit_eligible: provisionalCreditEligible,
    provisional_credit_issued: provisionalCreditEligible,
    provisional_credit_amount: provisionalCreditEligible
      ? disputedAmount
      : null,
    customer_max_liability_amount: customerMaxLiabilityAmount,
    card_action: cardAction,
    is_fraud_category: isFraudCategory,
    pin_shared_voluntarily: pinSharedVoluntarily,
    submitted_at: FROZEN_TODAY,
    status: "OPEN",
  };
  const success = addToDb({
    db: state.db,
    dbName: "debit_card_disputes",
    recordId: disputeId,
    record: disputeRecord,
  });
  if (!success) {
    return "Error: Dispute may have already been filed for this transaction.";
  }
  const resultParts = [
    `Dispute ID: ${disputeId}`,
    `Transaction: ${transactionId}`,
    `Account: ${accountId}`,
    `Category: ${disputeCategory
      .replaceAll("_", " ")
      .split(" ")
      .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
      .join(" ")}`,
    `Disputed Amount: $${disputedAmount.toFixed(2)}`,
    `Card Action: ${cardAction
      .replaceAll("_", " ")
      .split(" ")
      .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
      .join(" ")}`,
  ];
  if (provisionalCreditEligible) {
    resultParts.push(
      `Provisional Credit: ISSUED - $${disputedAmount.toFixed(2)} credited within 10 business days per Regulation E.`
    );
  } else {
    resultParts.push(
      "Provisional Credit: Not eligible - see Debit Card Provisional Credit Guidelines for details."
    );
  }
  if (pinSharedVoluntarily) {
    resultParts.push(
      "WARNING: Customer indicated PIN was shared voluntarily. This may affect liability determination."
    );
  }
  if (isFraudCategory && !policeReportFiled && disputedAmount > 500) {
    resultParts.push(
      "RECOMMENDATION: For fraud disputes over $500, filing a police report is recommended."
    );
  }
  return resultParts.join("\n");
}

function setDebitCardRecurringBlock(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const cardId = str(kwargs.card_id);
  const blockRecurring = bool(kwargs.block_recurring);
  if (!cardId) {
    return "Error: Missing required parameter: card_id";
  }
  const debitCard = state.db.debit_cards?.data?.[cardId as string];
  if (!debitCard || typeof debitCard !== "object") {
    return `Error: Debit card '${cardId}' not found.`;
  }
  const cardStatus = String(debitCard.status ?? "").toUpperCase();
  if (cardStatus !== "ACTIVE") {
    return `Error: Cannot update recurring block settings for a card with status '${cardStatus}'. Card must be ACTIVE.`;
  }
  (debitCard as Record<string, unknown>).recurring_blocked = blockRecurring;
  if (blockRecurring) {
    return (
      `Recurring payments BLOCKED for debit card ${cardId}.\n` +
      `All recurring/subscription charges will be declined.\n` +
      `One-time purchases are not affected.\n` +
      `This change takes effect within 24 hours.`
    );
  }
  return (
    `Recurring payments UNBLOCKED for debit card ${cardId}.\n` +
    `Recurring/subscription charges will now be allowed.\n` +
    `This change takes effect within 24 hours.`
  );
}

function getDebitDisputeStatus(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const userId = str(kwargs.user_id);
  if (!userId) {
    return "Error: Missing required parameter: user_id";
  }
  const debitDisputesResult = queryDatabaseTool(
    state.db,
    "debit_card_disputes",
    JSON.stringify({ user_id: userId })
  );
  const resultParts = [
    "Debit card dispute history retrieved successfully.",
    "",
    "Executed: get_debit_dispute_status_7483",
    `Debit card dispute history for user ${userId}:`,
  ];
  const hasDisputes =
    !debitDisputesResult.includes("No records found") &&
    !debitDisputesResult.includes("No results found");
  if (hasDisputes) {
    resultParts.push(debitDisputesResult);
  } else {
    resultParts.push("\nNo debit card disputes found for this user.");
  }
  return resultParts.join("\n");
}

function getAtmDepositImages(
  state: BankingEnvState,
  kwargs: Record<string, unknown>
): string {
  const transactionId = str(kwargs.transaction_id);
  if (!transactionId) {
    return "Error: Missing required parameter: transaction_id";
  }
  const txn =
    state.db.bank_account_transaction_history?.data?.[transactionId as string];
  if (!txn || typeof txn !== "object") {
    return `Error: Transaction '${transactionId}' not found.`;
  }
  const txnType = String(txn.type ?? "");
  if (txnType !== "atm_deposit") {
    return `Error: Transaction '${transactionId}' is not an ATM deposit. This tool only works for ATM deposit transactions.`;
  }
  const description = String(txn.description ?? "");
  if (
    !description.toUpperCase().includes("RHO-BANK") &&
    !description.toUpperCase().includes("RHOBANK")
  ) {
    return `Error: Transaction '${transactionId}' is from a third-party ATM. Deposit images are only available for Rho-Bank ATM deposits. For third-party ATM disputes, a chargeback request must be submitted to the ATM network.`;
  }
  const depositImageData: Record<
    string,
    {
      atm_id: string;
      deposit_date: string;
      envelope_contents: string;
      verification_notes: string;
    }
  > = {
    btxn_834027370c20: {
      atm_id: "ATM #3921",
      deposit_date: String(txn.date ?? "Unknown"),
      envelope_contents: `
=== ATM DEPOSIT ENVELOPE SCAN ===
Envelope ID: ENV-2025-3921-00923
Deposit Time: 3:47 PM CST
ATM Location: Rho-Bank ATM #3921, Cedar Lane Branch, Austin, TX

--- ENVELOPE CONTENTS ---
Item 1: Personal Check
  - Check Number: 7284
  - Drawn On: First Texas Bank
  - Payee: Derek Yamamoto
  - Amount: $875.00
  - Memo: "October rent refund"
  - Signature: Present and legible
  - Date on Check: 11/03/2025

Item 2: Cash
  - Denomination breakdown:
    * 2 x $100 bills = $200.00
    * 3 x $20 bills = $60.00
  - Total Cash: $260.00

--- DEPOSIT SUMMARY ---
Check Total: $875.00
Cash Total: $260.00
GRAND TOTAL: $1,135.00

--- MACHINE RECORD ---
Amount Recorded by ATM: $385.00
DISCREPANCY DETECTED: $750.00 difference

--- IMAGE QUALITY ---
Envelope scan: CLEAR
Check front: CLEAR
Check back (endorsement): CLEAR - endorsed "For Deposit Only - Derek Yamamoto"
Cash image: CLEAR - bills visible and countable
`,
      verification_notes:
        "Images clearly show deposit contents totaling $1,135.00. ATM machine recorded only $385.00. Discrepancy of $750.00 confirmed via image review.",
    },
    btxn_test_deposit_001: {
      atm_id: "ATM #3921",
      deposit_date: String(txn.date ?? "Unknown"),
      envelope_contents: `
=== ATM DEPOSIT ENVELOPE SCAN ===
Envelope ID: ENV-2025-3921-00847
Deposit Time: 2:34 PM EST
ATM Location: Rho-Bank ATM #3921, 742 Oak Avenue, Portland, OR

--- ENVELOPE CONTENTS ---
Item 1: Personal Check
  - Check Number: 4821
  - Drawn On: First National Bank
  - Payee: Linda Patterson
  - Amount: $875.00
  - Memo: "October rent refund"
  - Signature: Present and legible
  - Date on Check: 11/05/2025

Item 2: Cash
  - Denomination breakdown:
    * 2 x $100 bills = $200.00
    * 3 x $20 bills = $60.00
  - Total Cash: $260.00

--- DEPOSIT SUMMARY ---
Check Total: $875.00
Cash Total: $260.00
GRAND TOTAL: $1,135.00

--- MACHINE RECORD ---
Amount Recorded by ATM: $385.00
DISCREPANCY DETECTED: $750.00 difference

--- IMAGE QUALITY ---
Envelope scan: CLEAR
Check front: CLEAR
Check back (endorsement): CLEAR - endorsed "For Deposit Only - Linda Patterson"
Cash image: CLEAR - bills visible and countable
`,
      verification_notes:
        "Images clearly show deposit contents totaling $1,135.00. ATM machine recorded only $385.00. Discrepancy of $750.00 confirmed via image review.",
    },
    btxn_test_atm_dep_partial: {
      atm_id: "ATM #5847",
      deposit_date: String(txn.date ?? "Unknown"),
      envelope_contents: `
=== ATM DEPOSIT ENVELOPE SCAN ===
Envelope ID: ENV-2025-5847-00293
Deposit Time: 10:15 AM EST

--- ENVELOPE CONTENTS ---
Item 1: Personal Check
  - Check Number: 7392
  - Amount: $500.00

--- DEPOSIT SUMMARY ---
Check Total: $500.00
Cash Total: $0.00
GRAND TOTAL: $500.00

--- MACHINE RECORD ---
Amount Recorded by ATM: $500.00
No discrepancy detected.
`,
      verification_notes:
        "Images confirm deposit matches recorded amount. No discrepancy found.",
    },
  };
  if (transactionId in depositImageData) {
    const data = depositImageData[transactionId]!;
    const result = `ATM Deposit Image Retrieval Results
=====================================
Transaction ID: ${transactionId}
ATM: ${data.atm_id}
Deposit Date: ${data.deposit_date}

${data.envelope_contents}

--- VERIFICATION NOTES ---
${data.verification_notes}
`;
    return result;
  }
  return `ATM Deposit Image Retrieval Results
=====================================
Transaction ID: ${transactionId}
ATM: ${description}
Deposit Date: ${txn.date ?? "Unknown"}
Amount Recorded: $${Math.abs(num(txn.amount)).toFixed(2)}

--- IMAGE STATUS ---
Status: IMAGES NOT AVAILABLE
Reason: Deposit images for this transaction have either expired (older than 90 days) or were not captured by the ATM system.

For deposits without available images, the dispute will proceed based on customer statement and ATM journal records only. Investigation timeline may be extended.
`;
}

export function registerDisputeTools(): void {
  registerDiscoverableAgentTool({
    name: "update_transaction_rewards_3847",
    description:
      "Update the rewards_earned field on a credit card transaction.",
    params: [
      {
        name: "transaction_id",
        type: "string",
        optional: false,
        description: "The unique identifier for the transaction to update",
      },
      {
        name: "new_rewards_earned",
        type: "string",
        optional: false,
        description: "The corrected rewards value (e.g., '6300 points')",
      },
    ],
    handler: updateTransactionRewards,
  });
  registerDiscoverableAgentTool({
    name: "file_credit_card_transaction_dispute_4829",
    description: "File a formal dispute for a credit card transaction.",
    params: [
      {
        name: "transaction_id",
        type: "string",
        optional: false,
        description: "The unique identifier for the transaction being disputed",
      },
      {
        name: "card_action",
        type: "string",
        optional: false,
        description:
          "Flag indicating the card's status. Must be one of: 'keep_active', 'cancel_and_reissue'",
      },
      {
        name: "card_last_4_digits",
        type: "string",
        optional: false,
        description: "Last 4 digits of the credit card number",
      },
      {
        name: "full_name",
        type: "string",
        optional: false,
        description: "Full legal name of the cardholder",
      },
      {
        name: "user_id",
        type: "string",
        optional: false,
        description: "The user's unique identifier in the system",
      },
      {
        name: "phone",
        type: "string",
        optional: false,
        description: "Contact phone number",
      },
      {
        name: "email",
        type: "string",
        optional: false,
        description: "Contact email address",
      },
      {
        name: "address",
        type: "string",
        optional: false,
        description: "Contact mailing address",
      },
      {
        name: "contacted_merchant",
        type: "boolean",
        optional: false,
        description:
          "Whether the user attempted to resolve the issue with the merchant first",
      },
      {
        name: "purchase_date",
        type: "string",
        optional: false,
        description: "Date when the purchase was made, format MM/DD/YYYY",
      },
      {
        name: "issue_noticed_date",
        type: "string",
        optional: false,
        description: "Date when the user noticed the issue, format MM/DD/YYYY",
      },
      {
        name: "dispute_reason",
        type: "string",
        optional: false,
        description:
          "Reason for the dispute. Must be one of: unauthorized_fraudulent_charge, duplicate_charge, incorrect_amount, goods_services_not_received, goods_services_not_as_described, canceled_subscription_still_charging, refund_never_processed",
      },
      {
        name: "resolution_requested",
        type: "string",
        optional: false,
        description:
          "Resolution being requested. Must be one of: 'full_refund', 'partial_refund'",
      },
      {
        name: "eligible_for_provisional_credit",
        type: "boolean",
        optional: false,
        description: "Whether the user is eligible for provisional credit",
      },
      {
        name: "partial_refund_amount",
        type: "number",
        optional: true,
        description:
          "Amount requested for partial refund (required only if resolution_requested is 'partial_refund')",
      },
    ],
    handler: fileCreditCardTransactionDispute,
  });
  registerDiscoverableAgentTool({
    name: "file_debit_card_transaction_dispute_6281",
    description:
      "File a formal dispute for a debit card transaction under Regulation E.",
    params: [
      {
        name: "transaction_id",
        type: "string",
        optional: false,
        description: "The unique identifier for the transaction being disputed",
      },
      {
        name: "account_id",
        type: "string",
        optional: false,
        description: "The checking account ID linked to the debit card",
      },
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID",
      },
      {
        name: "user_id",
        type: "string",
        optional: false,
        description: "The user's unique identifier in the system",
      },
      {
        name: "dispute_category",
        type: "string",
        optional: false,
        description:
          "Category of the dispute. Must be one of: unauthorized_transaction, atm_cash_discrepancy, atm_deposit_not_credited, duplicate_charge, incorrect_amount, goods_services_not_received, recurring_charge_after_cancellation, card_present_fraud, card_not_present_fraud",
      },
      {
        name: "transaction_date",
        type: "string",
        optional: false,
        description: "Date when the transaction occurred, format MM/DD/YYYY",
      },
      {
        name: "discovery_date",
        type: "string",
        optional: false,
        description:
          "Date when the user first noticed the issue, format MM/DD/YYYY",
      },
      {
        name: "disputed_amount",
        type: "number",
        optional: false,
        description: "The dollar amount being disputed",
      },
      {
        name: "transaction_type",
        type: "string",
        optional: false,
        description:
          "Type of transaction. Must be one of: pin_purchase, signature_purchase, online_purchase, atm_withdrawal, atm_deposit, recurring_payment, person_to_person",
      },
      {
        name: "card_in_possession",
        type: "boolean",
        optional: false,
        description:
          "Whether the customer still has their physical debit card in their possession",
      },
      {
        name: "pin_compromised",
        type: "string",
        optional: false,
        description:
          "Whether the customer's PIN may have been compromised. Must be one of: 'yes_shared', 'yes_observed', 'no', 'unknown'",
      },
      {
        name: "contacted_merchant",
        type: "boolean",
        optional: false,
        description:
          "Whether the user attempted to resolve the issue with the merchant first",
      },
      {
        name: "police_report_filed",
        type: "boolean",
        optional: false,
        description:
          "Whether a police report has been filed (recommended for fraud over $500)",
      },
      {
        name: "written_statement_provided",
        type: "boolean",
        optional: false,
        description:
          "Whether the customer has provided a written statement describing what happened",
      },
      {
        name: "provisional_credit_eligible",
        type: "boolean",
        optional: false,
        description: "Whether the user is eligible for provisional credit",
      },
      {
        name: "customer_max_liability_amount",
        type: "number",
        optional: false,
        description:
          "The maximum dollar amount the customer could be liable for. Use -1 for unlimited liability.",
      },
      {
        name: "card_action",
        type: "string",
        optional: false,
        description:
          "Action to take on the card. Must be one of: 'keep_active', 'freeze_pending_investigation', 'close_and_reissue'",
      },
    ],
    handler: fileDebitCardTransactionDispute,
  });
  registerDiscoverableAgentTool({
    name: "set_debit_card_recurring_block_7382",
    description: "Block or unblock all recurring payments on a debit card.",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to update",
      },
      {
        name: "block_recurring",
        type: "boolean",
        optional: false,
        description:
          "True to block all recurring payments, False to unblock/allow recurring payments",
      },
    ],
    handler: setDebitCardRecurringBlock,
  });
  registerDiscoverableAgentTool({
    name: "get_debit_dispute_status_7483",
    mutatesState: false,
    description:
      "Retrieve a user's debit card dispute history from the debit_card_disputes table.",
    params: [
      {
        name: "user_id",
        type: "string",
        optional: false,
        description: "The user's unique identifier in the system",
      },
    ],
    handler: getDebitDisputeStatus,
  });
  registerDiscoverableAgentTool({
    name: "get_atm_deposit_images_8473",
    mutatesState: false,
    description:
      "Retrieve ATM deposit envelope/check images for a specific ATM deposit transaction.",
    params: [
      {
        name: "transaction_id",
        type: "string",
        optional: false,
        description:
          "The transaction ID of the ATM deposit to retrieve images for",
      },
    ],
    handler: getAtmDepositImages,
  });
}
