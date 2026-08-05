import { createHash } from "node:crypto";

import { isDefinedAndNotNull } from "../../../internal/guards";
import { getFieldAsString } from "./field-access";
import { getTodayStr } from "./helpers";
import { generateDebitCardOrderId, generateDebitCardId } from "./ids";
import type { BankingEnvState } from "./registry";
import { registerDiscoverableAgentTool } from "./registry";

function validatePin(pin: string): string | null {
  if (!pin || !/^\d{4}$/.test(pin)) {
    return "PIN must be exactly 4 digits.";
  }
  const sequentialPins = [
    "0123",
    "1234",
    "2345",
    "3456",
    "4567",
    "5678",
    "6789",
    "9876",
    "8765",
    "7654",
    "6543",
    "5432",
    "4321",
    "3210",
  ];
  if (sequentialPins.includes(pin)) {
    return "PIN cannot be sequential (e.g., 1234). Please choose a more secure PIN.";
  }
  const uniqueDigits = new Set(pin).size;
  if (uniqueDigits === 1) {
    return "PIN cannot be all the same digit (e.g., 1111). Please choose a more secure PIN.";
  }
  return null;
}

function validateActivationCommon(opts: {
  args: Record<string, unknown>;
  state: BankingEnvState;
  allowedIssueReasons: readonly string[];
  toolName: string;
}): [string | null, Record<string, unknown> | null] {
  const { args, state, allowedIssueReasons, toolName } = opts;
  const cardId = getFieldAsString(args, "card_id");
  const last4Digits = getFieldAsString(args, "last_4_digits");
  const expirationDate = getFieldAsString(args, "expiration_date");
  const cvv = getFieldAsString(args, "cvv");
  const pin = getFieldAsString(args, "pin");
  if (!cardId || !last4Digits || !expirationDate || !cvv || !pin) {
    return [
      "Error: Missing required parameters. Required: card_id, last_4_digits, expiration_date, cvv, pin.",
      null,
    ];
  }
  const pinError = validatePin(pin);
  if (pinError) {
    return [`Error: ${pinError}`, null];
  }
  if (!/^\d{3}$/.test(cvv)) {
    return ["Error: CVV must be exactly 3 digits.", null];
  }
  if (!/^\d{4}$/.test(last4Digits)) {
    return ["Error: Last 4 digits must be exactly 4 digits.", null];
  }
  const cardsTable = state.db.debit_cards.data;
  if (!(cardId in cardsTable)) {
    return [`Error: Debit card '${cardId}' not found.`, null];
  }
  const card = cardsTable[cardId]!;
  const issueReason = (card.issue_reason as string) || "new_account";
  const issueReasonArray = [...allowedIssueReasons];
  if (!issueReasonArray.includes(issueReason)) {
    const reasonMap: Record<string, string> = {
      new_account: "activate_debit_card_8291",
      first_card: "activate_debit_card_8291",
      lost: "activate_debit_card_8292",
      stolen: "activate_debit_card_8292",
      fraud: "activate_debit_card_8292",
      expired: "activate_debit_card_8293",
      damaged: "activate_debit_card_8293",
      upgrade: "activate_debit_card_8293",
      bank_reissue: "activate_debit_card_8293",
    };
    const correctTool = reasonMap[issueReason] || "unknown";
    return [
      `Error: Wrong activation tool. This card has issue_reason='${issueReason}'. Please use ${correctTool} instead of ${toolName}.`,
      null,
    ];
  }
  if (card.status === "ACTIVE") {
    return [`Error: Debit card '${cardId}' is already active.`, null];
  }
  if (card.status !== "PENDING") {
    return [
      `Error: Debit card '${cardId}' cannot be activated. Current status: ${card.status}. Only PENDING cards can be activated.`,
      null,
    ];
  }
  if (card.last_4_digits !== last4Digits) {
    return [
      "Error: Card verification failed. The last 4 digits do not match our records.",
      null,
    ];
  }
  const accountId = card.account_id as string | undefined;
  if (accountId && accountId in state.db.accounts.data) {
    const account = state.db.accounts.data[accountId]!;
    if (account.status !== "OPEN") {
      return [
        `Error: The linked checking account '${accountId}' is no longer open. Card cannot be activated.`,
        null,
      ];
    }
  }
  return [null, card];
}

function orderDebitCard(opts: {
  state: BankingEnvState;
  accountId: string;
  userId: string;
  deliveryOption: string;
  deliveryFee: unknown;
  cardDesign: string;
  designFee: unknown;
  shippingAddress: string;
  excessReplacementFee: unknown;
}): string {
  const {
    state,
    accountId,
    userId,
    deliveryOption,
    deliveryFee,
    cardDesign,
    designFee,
    shippingAddress,
    excessReplacementFee,
  } = opts;
  let delivery = 0;
  let design = 0;
  let excess = 0;
  try {
    delivery = isDefinedAndNotNull(deliveryFee)
      ? Number.parseFloat(String(deliveryFee))
      : 0;
  } catch {
    return "Error: delivery_fee must be a number.";
  }
  try {
    design = isDefinedAndNotNull(designFee)
      ? Number.parseFloat(String(designFee))
      : 0;
  } catch {
    return "Error: design_fee must be a number.";
  }
  try {
    excess = isDefinedAndNotNull(excessReplacementFee)
      ? Number.parseFloat(String(excessReplacementFee))
      : 0;
  } catch {
    excess = 0;
  }
  if (
    !accountId ||
    !userId ||
    !deliveryOption ||
    !cardDesign ||
    !shippingAddress
  ) {
    return "Error: Missing required parameters. Required: account_id, user_id, delivery_option, delivery_fee, card_design, design_fee, shipping_address.";
  }
  const validDelivery = ["STANDARD", "EXPEDITED", "RUSH"];
  const deliveryUpper = String(deliveryOption).toUpperCase();
  if (!validDelivery.includes(deliveryUpper)) {
    return `Error: Invalid delivery_option. Must be one of: ${validDelivery.join(", ")}`;
  }
  const validDesign = ["CLASSIC", "PREMIUM", "CUSTOM"];
  const designUpper = String(cardDesign).toUpperCase();
  if (!validDesign.includes(designUpper)) {
    return `Error: Invalid card_design. Must be one of: ${validDesign.join(", ")}`;
  }
  const accountsTable = state.db.accounts.data;
  if (!(accountId in accountsTable)) {
    return `Error: Account '${accountId}' not found.`;
  }
  const account = accountsTable[accountId]!;
  if (account.class !== "checking") {
    return `Error: Debit cards can only be ordered for checking accounts. Account '${accountId}' is a ${account.class} account.`;
  }
  if (account.status !== "OPEN") {
    return `Error: Account must be OPEN. Account '${accountId}' has status: ${account.status}`;
  }
  if (account.user_id !== userId) {
    return `Error: Account '${accountId}' does not belong to user '${userId}'.`;
  }
  let currentHoldings = 0;
  try {
    const holdings = String(account.current_holdings || 0).replaceAll(",", "");
    currentHoldings = Number.parseFloat(holdings);
  } catch {
    currentHoldings = 0;
  }
  if (currentHoldings < 25) {
    return `Error: Account must have a minimum balance of $25. Current balance: $${currentHoldings.toFixed(2)}`;
  }
  const debitCardOrdersTable = state.db.debit_card_orders.data;
  for (const orderId in debitCardOrdersTable) {
    const order = debitCardOrdersTable[orderId]!;
    if (order.account_id === accountId && order.status === "PENDING") {
      return `Error: There is already a pending debit card order for account '${accountId}'.`;
    }
  }
  const debitCardsTable = state.db.debit_cards.data;
  let activeCardCount = 0;
  for (const cardId in debitCardsTable) {
    const card = debitCardsTable[cardId]!;
    if (card.account_id === accountId && card.status === "ACTIVE") {
      activeCardCount += 1;
    }
  }
  if (activeCardCount >= 1) {
    return `Error: Account '${accountId}' already has an active debit card. Maximum 1 active card per checking account.`;
  }
  const totalFee = delivery + design + excess;
  if (totalFee > 0 && currentHoldings < totalFee) {
    return `Error: Insufficient funds for fees. Total fees: $${totalFee.toFixed(2)}. Current balance: $${currentHoldings.toFixed(2)}`;
  }
  const deliveryTimes: Record<string, string> = {
    STANDARD: "7-10 business days",
    EXPEDITED: "3-5 business days",
    RUSH: "1-2 business days",
  };
  const expectedDelivery = deliveryTimes[deliveryUpper];
  const orderDate = getTodayStr();
  const orderId = generateDebitCardOrderId(accountId, userId, deliveryUpper);
  const orderRecord: Record<string, unknown> = {
    order_id: orderId,
    account_id: accountId,
    user_id: userId,
    delivery_option: deliveryUpper,
    card_design: designUpper,
    shipping_address: shippingAddress,
    delivery_fee: delivery,
    design_fee: design,
    excess_replacement_fee: excess,
    total_fee: totalFee,
    order_date: orderDate,
    expected_delivery: expectedDelivery,
    status: "PENDING",
  };
  state.db.debit_card_orders.data[orderId] = orderRecord;
  if (totalFee > 0) {
    const newBalance = currentHoldings - totalFee;
    account.current_holdings = newBalance.toFixed(2);
    const feeDescriptionParts: string[] = [];
    if (delivery > 0) {
      feeDescriptionParts.push(`Delivery $${delivery.toFixed(2)}`);
    }
    if (design > 0) {
      feeDescriptionParts.push(`Design $${design.toFixed(2)}`);
    }
    if (excess > 0) {
      feeDescriptionParts.push(`Excess Replacement $${excess.toFixed(0)}`);
    }
    const feeDescription = `DEBIT CARD ORDER FEE - ${feeDescriptionParts.join(", ")}`;
    const feeTxnId = `btxn_dcfee_${orderId.slice(-8)}`;
    const feeTransaction: Record<string, unknown> = {
      transaction_id: feeTxnId,
      account_id: accountId,
      date: orderDate,
      description: feeDescription,
      amount: -totalFee,
      type: "debit_card_fee",
      status: "posted",
    };
    state.db.bank_account_transaction_history.data[feeTxnId] = feeTransaction;
  }
  const cardId = generateDebitCardId(accountId, userId, orderDate);
  let cardholder = "CARDHOLDER";
  const usersTable = state.db.users.data;
  if (userId in usersTable) {
    const user = usersTable[userId]!;
    cardholder = ((user.name as string) || "CARDHOLDER").toUpperCase();
  }
  let last4Digits = "0000";
  let cvvDigits = "000";
  try {
    const cardDetailsSeed = `card_details:${cardId}`;
    const hash1 = createHash("sha256")
      .update(`${cardDetailsSeed}:last4`)
      .digest("hex");
    const hash2 = createHash("sha256")
      .update(`${cardDetailsSeed}:cvv`)
      .digest("hex");
    const digits1 = hash1.replaceAll(/\D/g, "");
    const digits2 = hash2.replaceAll(/\D/g, "");
    last4Digits = (digits1.slice(0, 4) || "0000").padEnd(4, "0");
    cvvDigits = (digits2.slice(0, 3) || "000").padEnd(3, "0");
  } catch {
    last4Digits = "0000";
    cvvDigits = "000";
  }
  let expirationDate = "12/31/2029";
  try {
    const parts = orderDate.split("/");
    if (parts.length === 3) {
      const month = parts[0]!;
      const year = parts[2]!;
      const yearInt = Number.parseInt(year, 10) + 4;
      let day = "31";
      if (["01", "03", "05", "07", "08", "10", "12"].includes(month)) {
        day = "31";
      } else if (month === "02") {
        day = "28";
      } else {
        day = "30";
      }
      expirationDate = `${month}/${day}/${yearInt}`;
    }
  } catch {
    expirationDate = "12/31/2029";
  }
  let issueReason = "new_account";
  const existingCards: Record<string, unknown>[] = [];
  for (const cid in debitCardsTable) {
    const c = debitCardsTable[cid]!;
    if (c.account_id === accountId) {
      existingCards.push(c);
    }
  }
  const closedCard = existingCards.find((c) => c.status === "CLOSED");
  if (closedCard) {
    const closureReason = String(closedCard.closure_reason || "first_card");
    if (
      ["lost", "stolen", "fraud", "fraud_suspected"].includes(closureReason)
    ) {
      issueReason =
        closureReason === "fraud_suspected" ? "fraud" : closureReason;
    } else {
      issueReason = "first_card";
    }
  } else if (existingCards.length > 0) {
    issueReason = "first_card";
  }
  const cardRecord: Record<string, unknown> = {
    card_id: cardId,
    account_id: accountId,
    user_id: userId,
    cardholder_name: cardholder,
    last_4_digits: last4Digits,
    cvv: cvvDigits,
    status: "PENDING",
    issue_date: orderDate,
    expiration_date: expirationDate,
    card_design: designUpper,
    issue_reason: issueReason,
  };
  state.db.debit_cards.data[cardId] = cardRecord;
  const resultParts: string[] = [
    "Debit Card Order Confirmed",
    `Order ID: ${orderId}`,
    `Card ID: ${cardId}`,
    `Linked Account: ${accountId}`,
    `Delivery Option: ${deliveryUpper}`,
    `Card Design: ${designUpper}`,
    `Shipping Address: ${shippingAddress}`,
    `Expected Delivery: ${expectedDelivery}`,
    "",
    "Note: Card will arrive with status PENDING. Customer must call to activate after receiving the card.",
  ];
  if (totalFee > 0) {
    const feeDetails: string[] = [];
    if (delivery > 0) {
      feeDetails.push(`Delivery: $${delivery.toFixed(2)}`);
    }
    if (design > 0) {
      feeDetails.push(`Design: $${design.toFixed(2)}`);
    }
    if (excess > 0) {
      feeDetails.push(`Excess Replacement: $${excess.toFixed(0)}`);
    }
    resultParts.push(
      `Total Fees: $${totalFee.toFixed(2)} (${feeDetails.join(", ")}) - CHARGED to account ${accountId}`
    );
    resultParts.push(
      `New Account Balance: $${(currentHoldings - totalFee).toFixed(2)}`
    );
  } else {
    resultParts.push("Total Fees: $0 (No additional charges)");
  }
  return resultParts.join("\n");
}

export function registerDebitTools(): void {
  registerDiscoverableAgentTool({
    name: "order_debit_card_5739",
    description: "Order a new debit card for a customer's checking account.",
    params: [
      {
        name: "account_id",
        type: "string",
        optional: false,
        description: "The checking account ID to link the debit card to",
      },
      {
        name: "user_id",
        type: "string",
        optional: false,
        description: "The customer's unique identifier",
      },
      {
        name: "delivery_option",
        type: "string",
        optional: false,
        description: "Shipping speed: STANDARD, EXPEDITED, or RUSH",
      },
      {
        name: "delivery_fee",
        type: "number",
        optional: false,
        description: "Fee to charge for delivery in dollars",
      },
      {
        name: "card_design",
        type: "string",
        optional: false,
        description: "Card design: CLASSIC, PREMIUM, or CUSTOM",
      },
      {
        name: "design_fee",
        type: "number",
        optional: false,
        description: "Fee to charge for card design in dollars",
      },
      {
        name: "shipping_address",
        type: "string",
        optional: false,
        description: "Full shipping address for card delivery",
      },
      {
        name: "excess_replacement_fee",
        type: "number",
        optional: true,
        description: "Fee for exceeding replacement limit, if applicable",
      },
    ],
    handler: (state, kwargs) =>
      orderDebitCard({
        state,
        accountId: String(kwargs.account_id),
        userId: String(kwargs.user_id),
        deliveryOption: String(kwargs.delivery_option),
        deliveryFee: kwargs.delivery_fee,
        cardDesign: String(kwargs.card_design),
        designFee: kwargs.design_fee,
        shippingAddress: String(kwargs.shipping_address),
        excessReplacementFee: kwargs.excess_replacement_fee,
      }),
  });
  registerDiscoverableAgentTool({
    name: "activate_debit_card_8291",
    description:
      "Activate a NEW debit card for a customer. Use ONLY for first-time cards on a checking account (issue_reason = 'new_account' or 'first_card').",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to activate",
      },
      {
        name: "last_4_digits",
        type: "string",
        optional: false,
        description: "Last 4 digits of the card number (for verification)",
      },
      {
        name: "expiration_date",
        type: "string",
        optional: false,
        description: "Card expiration date in MM/YY format",
      },
      {
        name: "cvv",
        type: "string",
        optional: false,
        description: "3-digit CVV from the back of the card",
      },
      {
        name: "pin",
        type: "string",
        optional: false,
        description: "4-digit PIN chosen by the customer",
      },
    ],
    handler: (state, kwargs) => {
      const [error, card] = validateActivationCommon({
        args: kwargs,
        state,
        allowedIssueReasons: ["new_account", "first_card"],
        toolName: "activate_debit_card_8291",
      });
      if (error) {
        return error;
      }
      const cardId = String(kwargs.card_id);
      const accountId = String(card!.account_id);
      card!.status = "ACTIVE";
      card!.activated_date = getTodayStr();
      const deactivatedCards: string[] = [];
      const cardsTable = state.db.debit_cards.data;
      for (const otherId in cardsTable) {
        const other = cardsTable[otherId]!;
        if (
          otherId !== cardId &&
          other.account_id === accountId &&
          other.status === "ACTIVE"
        ) {
          other.status = "DEACTIVATED";
          other.deactivated_date = getTodayStr();
          other.deactivation_reason = "New card activated";
          deactivatedCards.push(otherId);
        }
      }
      const resultParts: string[] = [
        "New Debit Card Activation Successful",
        `Card ID: ${cardId}`,
        "Status: ACTIVE",
        `Activation Date: ${getTodayStr()}`,
        "",
        "Your card is now ready to use at any ATM or point of sale terminal.",
        "For security, please sign the back of your card.",
      ];
      if (deactivatedCards.length > 0) {
        resultParts.push(
          `\nNote: Previous card(s) have been deactivated: ${deactivatedCards.join(", ")}`
        );
      }
      return resultParts.join("\n");
    },
  });
  registerDiscoverableAgentTool({
    name: "activate_debit_card_8292",
    description:
      "Activate a REPLACEMENT debit card. Use ONLY for cards replacing lost, stolen, or fraud-suspected cards.",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to activate",
      },
      {
        name: "last_4_digits",
        type: "string",
        optional: false,
        description: "Last 4 digits of the card number (for verification)",
      },
      {
        name: "expiration_date",
        type: "string",
        optional: false,
        description: "Card expiration date in MM/YY format",
      },
      {
        name: "cvv",
        type: "string",
        optional: false,
        description: "3-digit CVV from the back of the card",
      },
      {
        name: "pin",
        type: "string",
        optional: false,
        description: "4-digit PIN chosen by the customer",
      },
    ],
    handler: (state, kwargs) => {
      const [error, card] = validateActivationCommon({
        args: kwargs,
        state,
        allowedIssueReasons: ["lost", "stolen", "fraud"],
        toolName: "activate_debit_card_8292",
      });
      if (error) {
        return error;
      }
      const cardId = String(kwargs.card_id);
      const accountId = String(card!.account_id);
      const issueReason = String(card!.issue_reason || "lost");
      card!.status = "ACTIVE";
      card!.activated_date = getTodayStr();
      const deactivatedCards: string[] = [];
      const cardsTable = state.db.debit_cards.data;
      for (const otherId in cardsTable) {
        const other = cardsTable[otherId]!;
        if (
          otherId !== cardId &&
          other.account_id === accountId &&
          other.status === "ACTIVE"
        ) {
          other.status = "DEACTIVATED";
          other.deactivated_date = getTodayStr();
          other.deactivation_reason = `Replacement card activated (${issueReason})`;
          deactivatedCards.push(otherId);
        }
      }
      const resultParts: string[] = [
        "Replacement Debit Card Activation Successful",
        `Card ID: ${cardId}`,
        `Replacement Reason: ${issueReason
          .replaceAll("_", " ")
          .split(" ")
          .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
          .join(" ")}`,
        "Status: ACTIVE",
        `Activation Date: ${getTodayStr()}`,
        "",
        "Your replacement card is now ready to use.",
        "",
        "IMPORTANT SECURITY REMINDERS:",
        "- Please review your recent transactions for any unauthorized charges",
        "- Report any suspicious activity immediately",
      ];
      if (issueReason === "fraud") {
        resultParts.push(
          "- Since fraud was suspected, we recommend changing your online banking password"
        );
      }
      if (deactivatedCards.length > 0) {
        resultParts.push(
          `\nPrevious card(s) have been deactivated for security: ${deactivatedCards.join(", ")}`
        );
      }
      return resultParts.join("\n");
    },
  });
  registerDiscoverableAgentTool({
    name: "activate_debit_card_8293",
    description:
      "Activate a REISSUED debit card. Use ONLY for cards reissued due to expiration, damage, upgrade, or bank-initiated replacement.",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to activate",
      },
      {
        name: "last_4_digits",
        type: "string",
        optional: false,
        description: "Last 4 digits of the card number (for verification)",
      },
      {
        name: "expiration_date",
        type: "string",
        optional: false,
        description: "Card expiration date in MM/YY format",
      },
      {
        name: "cvv",
        type: "string",
        optional: false,
        description: "3-digit CVV from the back of the card",
      },
      {
        name: "pin",
        type: "string",
        optional: false,
        description: "4-digit PIN chosen by the customer",
      },
    ],
    handler: (state, kwargs) => {
      const [error, card] = validateActivationCommon({
        args: kwargs,
        state,
        allowedIssueReasons: ["expired", "damaged", "upgrade", "bank_reissue"],
        toolName: "activate_debit_card_8293",
      });
      if (error) {
        return error;
      }
      const cardId = String(kwargs.card_id);
      const accountId = String(card!.account_id);
      const issueReason = String(card!.issue_reason || "expired");
      card!.status = "ACTIVE";
      card!.activated_date = getTodayStr();
      const oldCardsWithGrace: string[] = [];
      const cardsTable = state.db.debit_cards.data;
      for (const otherId in cardsTable) {
        const other = cardsTable[otherId]!;
        if (
          otherId !== cardId &&
          other.account_id === accountId &&
          other.status === "ACTIVE"
        ) {
          other.status = "GRACE_PERIOD";
          other.grace_period_ends = getTodayStr();
          other.deactivation_reason = `Reissued card activated (${issueReason})`;
          oldCardsWithGrace.push(otherId);
        }
      }
      const resultParts: string[] = [
        "Reissued Debit Card Activation Successful",
        `Card ID: ${cardId}`,
        `Reissue Reason: ${issueReason.replaceAll("_", " ").replace(/^./, (c) => c.toUpperCase())}`,
        "Status: ACTIVE",
        `Activation Date: ${getTodayStr()}`,
        "",
        "Your reissued card is now ready to use.",
      ];
      if (oldCardsWithGrace.length > 0) {
        resultParts.push(
          `\nNote: Your previous card(s) (${oldCardsWithGrace.join(", ")}) will remain active for 24 hours as a grace period.`
        );
        resultParts.push(
          "After 24 hours, the old card(s) will be automatically deactivated."
        );
      }
      if (["expired", "bank_reissue"].includes(issueReason)) {
        resultParts.push(
          "\nReminder: If your card number changed, please update any recurring payments with your new card details."
        );
      }
      return resultParts.join("\n");
    },
  });
  registerDiscoverableAgentTool({
    name: "close_debit_card_4721",
    description: "Close or cancel a debit card permanently.",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to close",
      },
      {
        name: "reason",
        type: "string",
        optional: false,
        description:
          "Reason for closing: lost, stolen, fraud_suspected, damaged, no_longer_needed, or account_closing",
      },
    ],
    handler: (state, kwargs) => {
      const cardId = String(kwargs.card_id);
      const reason = String(kwargs.reason).toLowerCase();
      if (!cardId || !reason) {
        return "Error: Missing required parameters. Required: card_id, reason.";
      }
      const validReasons = [
        "lost",
        "stolen",
        "fraud_suspected",
        "damaged",
        "no_longer_needed",
        "account_closing",
      ];
      if (!validReasons.includes(reason)) {
        return `Error: Invalid reason. Must be one of: ${validReasons.join(", ")}`;
      }
      const cardsTable = state.db.debit_cards.data;
      if (!(cardId in cardsTable)) {
        return `Error: Debit card '${cardId}' not found.`;
      }
      const card = cardsTable[cardId]!;
      if (!["ACTIVE", "PENDING"].includes(String(card.status))) {
        return `Error: Debit card '${cardId}' cannot be closed. Current status: ${card.status}. Only ACTIVE or PENDING cards can be closed.`;
      }
      const previousStatus = card.status;
      card.status = "CLOSED";
      card.closed_date = getTodayStr();
      card.closure_reason = reason;
      const resultParts: string[] = [
        "Debit Card Closed Successfully",
        `Card ID: ${cardId}`,
        `Previous Status: ${previousStatus}`,
        "New Status: CLOSED",
        `Closure Reason: ${reason.replaceAll("_", " ").replace(/^./, (c) => c.toUpperCase())}`,
        `Closure Date: ${getTodayStr()}`,
        "",
      ];
      if (["lost", "stolen", "fraud_suspected"].includes(reason)) {
        resultParts.push(
          "IMPORTANT: This card has been immediately deactivated for security."
        );
        resultParts.push("Any pending transactions may still be processed.");
        if (reason === "fraud_suspected") {
          resultParts.push(
            "Please advise the customer to review recent transactions and file disputes for any unauthorized charges."
          );
          resultParts.push(
            "Also recommend changing their online banking password."
          );
        }
      }
      resultParts.push("");
      resultParts.push(
        "Note: This card cannot be reactivated. If the customer needs a new card, they can order one through the standard ordering process."
      );
      resultParts.push(
        "Any recurring payments linked to this card will need to be updated with new payment information."
      );
      return resultParts.join("\n");
    },
  });
  registerDiscoverableAgentTool({
    name: "freeze_debit_card_3892",
    description:
      "Temporarily freeze a debit card. The card can be unfrozen later.",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to freeze",
      },
    ],
    handler: (state, kwargs) => {
      const cardId = String(kwargs.card_id);
      if (!cardId) {
        return "Error: Missing required parameter: card_id.";
      }
      const cardsTable = state.db.debit_cards.data;
      if (!(cardId in cardsTable)) {
        return `Error: Debit card '${cardId}' not found.`;
      }
      const card = cardsTable[cardId]!;
      if (card.status === "FROZEN") {
        return `Error: Debit card '${cardId}' is already frozen.`;
      }
      if (card.status !== "ACTIVE") {
        return `Error: Debit card '${cardId}' cannot be frozen. Current status: ${card.status}. Only ACTIVE cards can be frozen.`;
      }
      card.status = "FROZEN";
      card.frozen_date = getTodayStr();
      const resultParts: string[] = [
        "Debit Card Frozen Successfully",
        `Card ID: ${cardId}`,
        "Status: FROZEN",
        `Frozen Date: ${getTodayStr()}`,
        "",
        "While frozen:",
        "- All new purchase transactions will be declined",
        "- Recurring payments and subscriptions will be declined",
        "- Pending transactions already authorized may still process",
        "",
        "To unfreeze, the customer can call customer service or use the mobile app.",
        "If the card is confirmed lost or stolen, recommend closing the card permanently instead.",
      ];
      return resultParts.join("\n");
    },
  });
  registerDiscoverableAgentTool({
    name: "unfreeze_debit_card_3893",
    description: "Unfreeze a previously frozen debit card.",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to unfreeze",
      },
    ],
    handler: (state, kwargs) => {
      const cardId = String(kwargs.card_id);
      if (!cardId) {
        return "Error: Missing required parameter: card_id.";
      }
      const cardsTable = state.db.debit_cards.data;
      if (!(cardId in cardsTable)) {
        return `Error: Debit card '${cardId}' not found.`;
      }
      const card = cardsTable[cardId]!;
      if (card.status === "ACTIVE") {
        return `Error: Debit card '${cardId}' is already active.`;
      }
      if (card.status !== "FROZEN") {
        return `Error: Debit card '${cardId}' cannot be unfrozen. Current status: ${card.status}. Only FROZEN cards can be unfrozen.`;
      }
      const accountId = card.account_id as string | undefined;
      if (accountId && accountId in state.db.accounts.data) {
        const account = state.db.accounts.data[accountId]!;
        if (account.status !== "OPEN") {
          return `Error: The linked checking account '${accountId}' is no longer open. Card cannot be unfrozen.`;
        }
      }
      card.status = "ACTIVE";
      card.unfrozen_date = getTodayStr();
      const resultParts: string[] = [
        "Debit Card Unfrozen Successfully",
        `Card ID: ${cardId}`,
        "Status: ACTIVE",
        `Unfrozen Date: ${getTodayStr()}`,
        "",
        "The card is now active and ready to use immediately.",
        "All transactions will process normally.",
      ];
      return resultParts.join("\n");
    },
  });
  registerDiscoverableAgentTool({
    name: "clear_debit_card_fraud_alert_4892",
    description: "Clear a fraud alert or velocity block on a debit card.",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to clear the alert/block for",
      },
      {
        name: "reason",
        type: "string",
        optional: false,
        description: "Reason for clearing: customer_verified or velocity_clear",
      },
    ],
    handler: (state, kwargs) => {
      const cardId = String(kwargs.card_id);
      const reason = String(kwargs.reason);
      if (!cardId) {
        return "Error: Missing required parameter: card_id.";
      }
      if (!reason) {
        return "Error: Missing required parameter: reason.";
      }
      const validReasons = ["customer_verified", "velocity_clear"];
      if (!validReasons.includes(reason)) {
        return `Error: Invalid reason '${reason}'. Must be one of: ${validReasons.join(", ")}`;
      }
      const cardsTable = state.db.debit_cards.data;
      if (!(cardId in cardsTable)) {
        return `Error: Debit card '${cardId}' not found.`;
      }
      const card = cardsTable[cardId]!;
      if (reason === "velocity_clear") {
        if (!card.velocity_blocked) {
          return `Error: Debit card '${cardId}' does not have an active velocity block.`;
        }
        card.velocity_blocked = false;
        card.velocity_cleared_date = getTodayStr();
        const resultParts: string[] = [
          "Velocity Block Cleared Successfully",
          `Card ID: ${cardId}`,
          `Cleared Date: ${getTodayStr()}`,
          "",
          "The card is now unblocked and ready for normal use.",
          "The velocity monitoring will continue - if the same unusual patterns recur,",
          "the card may be blocked again automatically.",
        ];
        return resultParts.join("\n");
      }
      if (reason === "customer_verified") {
        if (!card.fraud_alert_active) {
          return `Error: Debit card '${cardId}' does not have an active fraud alert.`;
        }
        const alertSource = card.alert_source as string | undefined;
        if (alertSource === "bank_initiated") {
          return "Error: BANK_INITIATED_ALERT - This fraud alert was initiated by the bank's fraud detection system and cannot be cleared by customer service agents. Please transfer the customer to the security team using transfer_to_human_agents.";
        }
        card.fraud_alert_active = false;
        card.alert_source = null;
        card.fraud_alert_cleared_date = getTodayStr();
        const resultParts: string[] = [
          "Fraud Alert Cleared Successfully",
          `Card ID: ${cardId}`,
          `Cleared Date: ${getTodayStr()}`,
          "",
          "The fraud alert has been removed from the card.",
          "All transactions will process normally.",
          "",
          "Remind the customer to review recent transactions and report any unauthorized charges.",
        ];
        return resultParts.join("\n");
      }
      return "Error: Unexpected error processing the request.";
    },
  });
  registerDiscoverableAgentTool({
    name: "reset_debit_card_pin_6284",
    description: "Reset a debit card PIN when the customer has forgotten it.",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to reset PIN for",
      },
      {
        name: "last_4_digits",
        type: "string",
        optional: false,
        description: "Last 4 digits of the card number (for verification)",
      },
      {
        name: "new_pin",
        type: "string",
        optional: false,
        description: "The new 4-digit PIN chosen by the customer",
      },
    ],
    handler: (state, kwargs) => {
      const cardId = String(kwargs.card_id);
      const last4Digits = String(kwargs.last_4_digits);
      const newPin = String(kwargs.new_pin);
      if (!cardId || !last4Digits || !newPin) {
        return "Error: Missing required parameters. Required: card_id, last_4_digits, new_pin.";
      }
      if (!/^\d{4}$/.test(last4Digits)) {
        return "Error: Last 4 digits must be exactly 4 digits.";
      }
      const pinError = validatePin(newPin);
      if (pinError) {
        return `Error: ${pinError}`;
      }
      const cardsTable = state.db.debit_cards.data;
      if (!(cardId in cardsTable)) {
        return `Error: Debit card '${cardId}' not found.`;
      }
      const card = cardsTable[cardId]!;
      if (card.last_4_digits !== last4Digits) {
        return "Error: Card verification failed. The last 4 digits do not match our records.";
      }
      if (card.status !== "ACTIVE") {
        return `Error: Cannot reset PIN. Card status is ${card.status}. Only ACTIVE cards can have their PIN reset.`;
      }
      card.pin_last_changed = getTodayStr();
      card.pin_locked = false;
      card.pin_attempts_remaining = 3;
      const resultParts: string[] = [
        "Debit Card PIN Reset Successfully",
        `Card ID: ${cardId}`,
        `PIN Changed: ${getTodayStr()}`,
        "",
        "The new PIN is effective immediately.",
        "Your card has been unlocked and is ready to use.",
        "Customer can use the new PIN for ATM withdrawals and point-of-sale transactions.",
        "",
        "Security reminder: Never share your PIN with anyone.",
      ];
      return resultParts.join("\n");
    },
  });
  registerDiscoverableAgentTool({
    name: "change_debit_card_pin_6285",
    description:
      "Change a debit card PIN when the customer knows their current PIN.",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to change PIN for",
      },
      {
        name: "current_pin",
        type: "string",
        optional: false,
        description: "The customer's current 4-digit PIN",
      },
      {
        name: "new_pin",
        type: "string",
        optional: false,
        description: "The new 4-digit PIN chosen by the customer",
      },
    ],
    handler: (state, kwargs) => {
      const cardId = String(kwargs.card_id);
      const currentPin = String(kwargs.current_pin);
      const newPin = String(kwargs.new_pin);
      if (!cardId || !currentPin || !newPin) {
        return "Error: Missing required parameters. Required: card_id, current_pin, new_pin.";
      }
      if (!/^\d{4}$/.test(currentPin)) {
        return "Error: Current PIN must be exactly 4 digits.";
      }
      const pinError = validatePin(newPin);
      if (pinError) {
        return `Error: ${pinError}`;
      }
      if (currentPin === newPin) {
        return "Error: New PIN must be different from current PIN.";
      }
      const cardsTable = state.db.debit_cards.data;
      if (!(cardId in cardsTable)) {
        return `Error: Debit card '${cardId}' not found.`;
      }
      const card = cardsTable[cardId]!;
      if (card.status !== "ACTIVE") {
        return `Error: Cannot change PIN. Card status is ${card.status}. Only ACTIVE cards can have their PIN changed.`;
      }
      card.pin_last_changed = getTodayStr();
      const resultParts: string[] = [
        "Debit Card PIN Changed Successfully",
        `Card ID: ${cardId}`,
        `PIN Changed: ${getTodayStr()}`,
        "",
        "The new PIN is effective immediately.",
        "Customer can use the new PIN for ATM withdrawals and point-of-sale transactions.",
        "",
        "Security reminder: Never share your PIN with anyone.",
      ];
      return resultParts.join("\n");
    },
  });
  registerDiscoverableAgentTool({
    name: "get_debit_cards_by_account_id_7823",
    mutatesState: false,
    description: "Retrieve all debit cards associated with a checking account.",
    params: [
      {
        name: "account_id",
        type: "string",
        optional: false,
        description: "The checking account ID to retrieve debit cards for",
      },
    ],
    handler: (state, kwargs) => {
      const accountId = String(kwargs.account_id);
      if (!accountId) {
        return "Error: Missing required parameter 'account_id'.";
      }
      const accountsTable = state.db.accounts.data;
      if (!(accountId in accountsTable)) {
        return `Error: Account '${accountId}' not found.`;
      }
      const account = accountsTable[accountId]!;
      const accountClass = String(account.class || "").toLowerCase();
      if (!["checking", "business_checking"].includes(accountClass)) {
        return `Error: Account '${accountId}' is not a checking account. Debit cards are only available for checking accounts.`;
      }
      const accountCards: Record<string, unknown>[] = [];
      const cardsTable = state.db.debit_cards.data;
      for (const cardId in cardsTable) {
        const card = cardsTable[cardId]!;
        if (card.account_id === accountId) {
          const cardInfo: Record<string, unknown> = { card_id: cardId };
          Object.assign(cardInfo, card);
          if (
            "last_4_digits" in cardInfo &&
            !("card_number_last_4" in cardInfo)
          ) {
            cardInfo.card_number_last_4 = cardInfo.last_4_digits;
            delete cardInfo.last_4_digits;
          }
          if ("issue_date" in cardInfo && !("date_issued" in cardInfo)) {
            cardInfo.date_issued = cardInfo.issue_date;
            delete cardInfo.issue_date;
          } else if (
            "created_date" in cardInfo &&
            !("date_issued" in cardInfo)
          ) {
            cardInfo.date_issued = cardInfo.created_date;
            delete cardInfo.created_date;
          }
          accountCards.push(cardInfo);
        }
      }
      if (accountCards.length === 0) {
        return `No debit cards found for account '${accountId}'.`;
      }
      accountCards.sort((a, b) =>
        (String(b.date_issued) || "").localeCompare(String(a.date_issued) || "")
      );
      return JSON.stringify(accountCards, null, 2);
    },
  });
  registerDiscoverableAgentTool({
    name: "request_temporary_debit_card_limit_increase_8374",
    description:
      "Request a temporary 24-hour increase to a debit card's daily ATM or purchase limit.",
    params: [
      {
        name: "card_id",
        type: "string",
        optional: false,
        description: "The debit card ID to increase limits for",
      },
      {
        name: "limit_type",
        type: "string",
        optional: false,
        description: "Type of limit to increase: atm or purchase",
      },
      {
        name: "new_limit",
        type: "number",
        optional: false,
        description: "The requested new temporary limit amount in dollars",
      },
    ],
    handler: (state, kwargs) => {
      const cardId = String(kwargs.card_id);
      const limitType = String(kwargs.limit_type);
      const newLimitRaw = kwargs.new_limit;
      if (!cardId) {
        return "Error: Missing required parameter: card_id.";
      }
      if (!limitType) {
        return "Error: Missing required parameter: limit_type.";
      }
      if (!["atm", "purchase"].includes(limitType)) {
        return `Error: Invalid limit_type '${limitType}'. Must be 'atm' or 'purchase'.`;
      }
      if (newLimitRaw === null || newLimitRaw === undefined) {
        return "Error: Missing required parameter: new_limit.";
      }
      const newLimit = Number(String(newLimitRaw));
      if (!Number.isFinite(newLimit) || !Number.isInteger(newLimit)) {
        return `Error: new_limit must be an integer, got '${newLimitRaw}'.`;
      }
      if (newLimit <= 0) {
        return "Error: new_limit must be a positive amount.";
      }
      const cardsTable = state.db.debit_cards.data;
      if (!(cardId in cardsTable)) {
        return `Error: Debit card '${cardId}' not found.`;
      }
      const card = cardsTable[cardId]!;
      if (card.status !== "ACTIVE") {
        return `Error: Debit card '${cardId}' is not active. Current status: ${card.status}. Only ACTIVE cards can have limit increases.`;
      }
      const accountId = card.account_id as string | undefined;
      if (!accountId || !(accountId in state.db.accounts.data)) {
        return `Error: Could not find linked account for debit card '${cardId}'.`;
      }
      const account = state.db.accounts.data[accountId]!;
      if (account.status !== "OPEN") {
        return `Error: The linked account '${accountId}' is not in good standing. Account status: ${account.status}.`;
      }
      let currentLimit: number | undefined;
      let limitField: string;
      let limitName: string;
      if (limitType === "atm") {
        currentLimit = card.daily_atm_limit as number | undefined;
        limitField = "daily_atm_limit";
        limitName = "Daily ATM Withdrawal Limit";
        if (currentLimit === null || currentLimit === undefined) {
          const accountLevel = String(account.level || "").toLowerCase();
          const defaultAtmLimits: Record<string, number> = {
            "blue account": 500,
            "green account": 600,
            "light blue account": 400,
            "green fee-free account": 500,
            "evergreen account": 750,
            "bluest account": 1500,
            "dark green account": 300,
            "gold years account": 600,
            "purple account": 1000,
          };
          if (accountLevel.includes("light green")) {
            return `Error: Light Green Account (teen checking) cards have policy-based limits that cannot be modified. The daily ATM withdrawal limit of $150 is fixed by account policy for safety reasons. The customer may request the parent/guardian to withdraw cash from their own account if needed.`;
          }
          currentLimit = defaultAtmLimits[accountLevel];
          if (currentLimit === undefined) {
            return `Error: Could not determine the default ATM limit for account type '${account.level}'. Please verify the account type.`;
          }
        }
      } else {
        currentLimit = card.daily_purchase_limit as number | undefined;
        limitField = "daily_purchase_limit";
        limitName = "Daily Purchase Limit";
        if (currentLimit === null || currentLimit === undefined) {
          return `Error: The card does not have a ${limitName.toLowerCase()} configured. This may be a restricted account type where limits are set by account policy and cannot be modified.`;
        }
      }
      const maxAllowed = Math.floor(currentLimit * 1.5);
      if (newLimit > maxAllowed) {
        return `Error: Requested limit $${newLimit} exceeds the maximum allowed temporary increase. Maximum temporary limit is $${maxAllowed} (150% of current $${currentLimit} limit).`;
      }
      if (newLimit <= currentLimit) {
        return `Error: Requested limit $${newLimit} is not higher than the current limit of $${currentLimit}.`;
      }
      const originalLimit = currentLimit;
      card[limitField] = newLimit;
      card[`temporary_${limitType}_limit_increase`] = true;
      card[`original_${limitType}_limit`] = originalLimit;
      card[`temporary_${limitType}_limit_expires`] = getTodayStr();
      const resultParts: string[] = [
        `Temporary ${limitName} Increase Granted Successfully`,
        `Card ID: ${cardId}`,
        `Previous Limit: $${originalLimit}`,
        `New Temporary Limit: $${newLimit}`,
        `Increase Amount: $${newLimit - originalLimit}`,
        "",
        "Important Information:",
        "- This temporary increase expires in 24 hours",
        `- After expiration, the limit will revert to $${originalLimit}`,
        "- Only one temporary increase is allowed per 24-hour period",
        "",
        "Note: If the customer is at a third-party (non-Rho-Bank) ATM, that ATM may have its own",
        "per-transaction or daily limits that Rho-Bank cannot override. The customer may need to",
        "use a Rho-Bank ATM or make multiple smaller withdrawals at third-party ATMs.",
      ];
      return resultParts.join("\n");
    },
  });
}
