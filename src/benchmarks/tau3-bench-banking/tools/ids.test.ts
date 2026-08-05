import { describe, it, expect } from "bun:test";

import {
  generateTransactionId,
  generateReferralId,
  generateApplicationId,
  generateVerificationId,
  generateUserDiscoverableToolId,
  generateUserDiscoverableToolCallId,
  generateDisputeId,
  generateReferralLinkId,
  generateAgentDiscoverableToolId,
  generateCreditCardOrderId,
  generateClosureReasonId,
  generateAccountFlagId,
  generateCreditLimitIncreaseRequestId,
  generateBankAccountTransactionId,
  generateDebitCardOrderId,
  generateDebitCardId,
  pythonSortedJsonDumps,
} from "./ids";
describe("ID generators", () => {
  describe("generateDisputeId", () => {
    it("generates deterministic dispute ID with expected hash", () => {
      const id = generateDisputeId("usr1", "txn1");
      expect(id).toMatch(/^dsp_[a-f0-9]{12}$/);
      expect(id).toBe("dsp_e1fd3e950495");
    });
  });
  describe("generateUserDiscoverableToolId", () => {
    it("generates deterministic user tool ID", () => {
      const id = generateUserDiscoverableToolId("test_tool");
      expect(id).toMatch(/^[a-f0-9]{16}$/);
      expect(id).toBe("0dcda015479f3314");
    });
  });
  describe("generateAgentDiscoverableToolId", () => {
    it("generates deterministic agent tool ID", () => {
      const id = generateAgentDiscoverableToolId("example_0000");
      expect(id).toMatch(/^[a-f0-9]{16}$/);
      expect(id).toBe("2cc6ceb8d9f34cca");
    });
  });
  describe("generateApplicationId", () => {
    it("generates deterministic application ID with boolean conversion", () => {
      const id = generateApplicationId({
        cardType: "PLATINUM",
        customerName: "John Doe",
        annualIncome: 85000,
        rhoaBankSubscription: true,
      });
      expect(id).toMatch(/^[a-f0-9]{16}$/);
      expect(id).toBe("1b3098341ba8111e");
    });
    it("generates different ID for false boolean", () => {
      const idTrue = generateApplicationId({
        cardType: "PLATINUM",
        customerName: "John Doe",
        annualIncome: 85000,
        rhoaBankSubscription: true,
      });
      const idFalse = generateApplicationId({
        cardType: "PLATINUM",
        customerName: "John Doe",
        annualIncome: 85000,
        rhoaBankSubscription: false,
      });
      expect(idTrue).not.toBe(idFalse);
    });
  });
  describe("generateVerificationId", () => {
    it("sanitizes timestamp: removes spaces, colons, dashes", () => {
      const id = generateVerificationId("usr1", "2025-11-14 03:40:00 EST");
      expect(id).toBe("usr1_20251114_034000_EST");
    });
  });
  describe("generateUserDiscoverableToolCallId", () => {
    it("matches the Python hash for sorted JSON arguments", () => {
      const args = { a: 1, b: 2 };
      const id = generateUserDiscoverableToolCallId("tool", args);
      expect(id).toBe("a73c858786b88826");
    });
    it("differs when arguments change", () => {
      const id1 = generateUserDiscoverableToolCallId("tool", { a: 1 });
      const id2 = generateUserDiscoverableToolCallId("tool", { a: 2 });
      expect(id1).not.toBe(id2);
    });
    it("same arguments produce same ID (deterministic)", () => {
      const args = { b: 1, a: 2 };
      const id1 = generateUserDiscoverableToolCallId("tool", args);
      const id2 = generateUserDiscoverableToolCallId("tool", { a: 2, b: 1 });
      expect(id1).toBe(id2);
    });
  });
  describe("pythonSortedJsonDumps", () => {
    it("sorts nested keys with Python separators", () => {
      expect(
        pythonSortedJsonDumps({ z: { b: 2, a: 1 }, a: [true, null] })
      ).toBe('{"a": [true, null], "z": {"a": 1, "b": 2}}');
    });
  });
  describe("generateTransactionId", () => {
    it("generates transaction ID with txn_ prefix", () => {
      const id = generateTransactionId({
        userId: "usr1",
        creditCardType: "VISA",
        merchantName: "Merchant",
        amount: 100.5,
        category: "food",
      });
      expect(id).toMatch(/^txn_[a-f0-9]{12}$/);
    });
    it("formats amount with .toFixed(2)", () => {
      const id1 = generateTransactionId({
        userId: "usr1",
        creditCardType: "VISA",
        merchantName: "Merchant",
        amount: 100,
        category: "food",
      });
      const id2 = generateTransactionId({
        userId: "usr1",
        creditCardType: "VISA",
        merchantName: "Merchant",
        amount: 100,
        category: "food",
      });
      expect(id1).toBe(id2);
    });
    it("includes date if provided", () => {
      const id1 = generateTransactionId({
        userId: "usr1",
        creditCardType: "VISA",
        merchantName: "Merchant",
        amount: 100,
        category: "food",
      });
      const id2 = generateTransactionId({
        userId: "usr1",
        creditCardType: "VISA",
        merchantName: "Merchant",
        amount: 100,
        category: "food",
        date: "2025-11-14",
      });
      expect(id1).not.toBe(id2);
    });
  });
  describe("generateReferralId", () => {
    it("generates 16-char hex referral ID", () => {
      const id = generateReferralId("usr1", "checking");
      expect(id).toMatch(/^[a-f0-9]{16}$/);
    });
  });
  describe("generateDisputeId", () => {
    it("generates dsp_ prefixed dispute ID", () => {
      const id = generateDisputeId("usr1", "txn_abc123");
      expect(id).toMatch(/^dsp_[a-f0-9]{12}$/);
    });
  });
  describe("generateReferralLinkId", () => {
    it("generates 16-char referral link ID", () => {
      const id = generateReferralLinkId("usr1", "Platinum Card");
      expect(id).toMatch(/^[a-f0-9]{16}$/);
    });
  });
  describe("generateCreditCardOrderId", () => {
    it("generates ccord_ prefixed order ID", () => {
      const id = generateCreditCardOrderId("cca_123", "usr1", "replacement");
      expect(id).toMatch(/^ccord_[a-f0-9]{12}$/);
    });
  });
  describe("generateClosureReasonId", () => {
    it("generates clsr_ prefixed closure reason ID", () => {
      const id = generateClosureReasonId("cca_123", "usr1");
      expect(id).toMatch(/^clsr_[a-f0-9]{12}$/);
    });
  });
  describe("generateAccountFlagId", () => {
    it("generates ccflag_ prefixed account flag ID", () => {
      const id = generateAccountFlagId("cca_123", "fraud_alert", "2025-12-31");
      expect(id).toMatch(/^ccflag_[a-f0-9]{12}$/);
    });
  });
  describe("generateCreditLimitIncreaseRequestId", () => {
    it("generates cli_ prefixed CLI request ID", () => {
      const id = generateCreditLimitIncreaseRequestId("cca_123", "usr1", 5000);
      expect(id).toMatch(/^cli_[a-f0-9]{12}$/);
    });
    it("formats amount with .toFixed(2)", () => {
      const id1 = generateCreditLimitIncreaseRequestId("cca_123", "usr1", 5000);
      const id2 = generateCreditLimitIncreaseRequestId("cca_123", "usr1", 5000);
      expect(id1).toBe(id2);
    });
  });
  describe("generateBankAccountTransactionId", () => {
    it("generates btxn_ prefixed bank transaction ID", () => {
      const id = generateBankAccountTransactionId({
        accountId: "chk_123",
        date: "11/14/2025",
        description: "ATM withdrawal",
        amount: -100,
        transactionType: "atm_withdrawal",
      });
      expect(id).toMatch(/^btxn_[a-f0-9]{12}$/);
    });
  });
  describe("generateDebitCardOrderId", () => {
    it("generates dcord_ prefixed debit card order ID", () => {
      const id = generateDebitCardOrderId("chk_123", "usr1", "STANDARD");
      expect(id).toMatch(/^dcord_[a-f0-9]{12}$/);
    });
  });
  describe("generateDebitCardId", () => {
    it("generates dbc_ prefixed debit card ID", () => {
      const id = generateDebitCardId("chk_123", "usr1", "2025-11-14");
      expect(id).toMatch(/^dbc_[a-f0-9]{12}$/);
    });
  });
  describe("Deterministic consistency", () => {
    it("same inputs produce same output across multiple calls", () => {
      const id1 = generateDisputeId("usr1", "txn1");
      const id2 = generateDisputeId("usr1", "txn1");
      expect(id1).toBe(id2);
    });
    it("different inputs produce different outputs", () => {
      const id1 = generateDisputeId("usr1", "txn1");
      const id2 = generateDisputeId("usr2", "txn1");
      expect(id1).not.toBe(id2);
    });
  });
});
