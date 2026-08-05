import { describe, it, expect } from "bun:test";
import assert from "node:assert/strict";

import { isRecord } from "../../../internal/guards";
import { makeEmptyBankingData } from "../environment";
import {
  queryDb,
  addToDb,
  updateRecordInDb,
  removeFromDb,
  listDatabases,
  queryDatabaseTool,
} from "./db-query";

function createTestDb() {
  const db = makeEmptyBankingData();
  db.users.data = {
    usr1: {
      user_id: "usr1",
      name: "Alice Johnson",
      email: "alice@example.com",
      status: "active",
      balance: 5000,
    },
    usr2: {
      user_id: "usr2",
      name: "Bob Smith",
      email: "bob@example.com",
      status: "inactive",
      balance: 2500,
    },
  };
  db.accounts.data = {
    acc1: {
      account_id: "acc1",
      user_id: "usr1",
      type: "checking",
      balance: 10000,
    },
    acc2: {
      account_id: "acc2",
      user_id: "usr2",
      type: "savings",
      balance: 50000,
    },
  };
  return db;
}
describe("db-query", () => {
  describe("queryDb", () => {
    it("returns empty array for non-existent database", () => {
      const db = createTestDb();
      const results = queryDb({ db, dbName: "nonexistent" });
      expect(results).toEqual([]);
    });
    it("queries with exact match (default eq operator)", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { status: "active" },
      });
      expect(results).toHaveLength(1);
      expect(results[0]?.user_id).toBe("usr1");
    });
    it("queries with explicit __eq operator", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { status__eq: "active" },
      });
      expect(results).toHaveLength(1);
    });
    it("queries with __ne operator", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { status__ne: "active" },
      });
      expect(results).toHaveLength(1);
      expect(results[0]?.user_id).toBe("usr2");
    });
    it("queries with __gt operator on numbers", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { balance__gt: 3000 },
      });
      expect(results).toHaveLength(1);
      expect(results[0]?.user_id).toBe("usr1");
    });
    it("returns non-match for type-mismatched __gt comparison", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { balance__gt: "abc" satisfies unknown },
      });
      expect(results).toHaveLength(0);
    });
    it("queries with __gte operator", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { balance__gte: 5000 },
      });
      expect(results).toHaveLength(1);
    });
    it("queries with __lt operator", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { balance__lt: 4000 },
      });
      expect(results).toHaveLength(1);
      expect(results[0]?.user_id).toBe("usr2");
    });
    it("queries with __lte operator", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { balance__lte: 2500 },
      });
      expect(results).toHaveLength(1);
    });
    it("queries with __contains on string", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { name__contains: "son" },
      });
      expect(results).toHaveLength(1);
      expect(results[0]?.name).toBe("Alice Johnson");
    });
    it("queries with __startswith", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { email__startswith: "alice" },
      });
      expect(results).toHaveLength(1);
    });
    it("queries with __endswith", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { email__endswith: "@example.com" },
      });
      expect(results).toHaveLength(2);
    });
    it("queries with __in operator", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { user_id__in: ["usr1", "usr3"] },
      });
      expect(results).toHaveLength(1);
      expect(results[0]?.user_id).toBe("usr1");
    });
    it("queries with __nin operator", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { user_id__nin: ["usr1"] },
      });
      expect(results).toHaveLength(1);
    });
    it("queries with returnIds option", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { status: "active" },
        options: { returnIds: true },
      });
      expect(results).toHaveLength(1);
      const first = results[0];
      assert(first);
      const [id, record] = first;
      expect(id).toBe("usr1");
      expect(record.user_id).toBe("usr1");
    });
    it("queries with limit option", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: {},
        options: { limit: 1 },
      });
      expect(results).toHaveLength(1);
    });
    it("applies multiple constraints with AND logic", () => {
      const db = createTestDb();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { status: "active", balance__gte: 4000 },
      });
      expect(results).toHaveLength(1);
      expect(results[0]?.user_id).toBe("usr1");
    });
  });
  describe("addToDb", () => {
    it("adds a record to database", () => {
      const db = createTestDb();
      const success = addToDb({
        db,
        dbName: "users",
        recordId: "usr3",
        record: {
          user_id: "usr3",
          name: "Charlie",
          email: "charlie@example.com",
          status: "active",
        },
      });
      expect(success).toBe(true);
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { user_id: "usr3" },
      });
      expect(results).toHaveLength(1);
    });
    it("returns false if table does not exist", () => {
      const db = createTestDb();
      const success = addToDb({
        db,
        dbName: "nonexistent",
        recordId: "id1",
        record: { field: "value" },
      });
      expect(success).toBe(false);
    });
    it("returns false if record already exists", () => {
      const db = createTestDb();
      const success = addToDb({
        db,
        dbName: "users",
        recordId: "usr1",
        record: { user_id: "usr1" },
      });
      expect(success).toBe(false);
    });
    it("stores __proto__ as an own property without polluting the prototype", () => {
      const db = createTestDb();
      const protoKey = ["__", "proto__"].join("");
      const success = addToDb({
        db,
        dbName: "users",
        recordId: protoKey,
        record: {
          user_id: protoKey,
          name: "Proto User",
        },
      });
      expect(success).toBe(true);
      expect(Object.hasOwn(db.users.data, protoKey)).toBe(true);
      expect(Reflect.get(Object.prototype, "user_id")).toBeUndefined();
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { user_id: protoKey },
      });
      expect(results).toHaveLength(1);
      expect(results[0]?.name).toBe("Proto User");
    });
  });
  describe("updateRecordInDb", () => {
    it("updates record fields", () => {
      const db = createTestDb();
      const [success, updated] = updateRecordInDb({
        db,
        dbName: "users",
        recordId: "usr1",
        updates: { status: "suspended" },
      });
      expect(success).toBe(true);
      expect(updated).not.toBeNull();
      expect(updated?.status).toBe("suspended");
    });
    it("returns false if record not found", () => {
      const db = createTestDb();
      const [success] = updateRecordInDb({
        db,
        dbName: "users",
        recordId: "usr999",
        updates: { status: "deleted" },
      });
      expect(success).toBe(false);
    });
    it("returns false if table does not exist", () => {
      const db = createTestDb();
      const [success] = updateRecordInDb({
        db,
        dbName: "nonexistent",
        recordId: "id1",
        updates: { field: "value" },
      });
      expect(success).toBe(false);
    });
    it("persists changes to database", () => {
      const db = createTestDb();
      updateRecordInDb({
        db,
        dbName: "users",
        recordId: "usr1",
        updates: { status: "suspended" },
      });
      const results = queryDb({
        db,
        dbName: "users",
        constraints: { user_id: "usr1" },
      });
      expect(results[0]?.status).toBe("suspended");
    });
    it("updates __proto__ as an own property without polluting Object.prototype", () => {
      const db = createTestDb();
      const protoKey = ["__", "proto__"].join("");
      const parsed: unknown = JSON.parse('{"__proto__":{"polluted":"yes"}}');
      assert(isRecord(parsed));
      const [success, updated] = updateRecordInDb({
        db,
        dbName: "users",
        recordId: "usr1",
        updates: parsed,
      });
      expect(success).toBe(true);
      expect(updated).not.toBeNull();
      expect(Object.hasOwn(updated ?? {}, protoKey)).toBe(true);
      expect(Reflect.get(Object.prototype, "polluted")).toBeUndefined();
      expect(Reflect.get(updated ?? {}, protoKey)).toEqual({ polluted: "yes" });
    });
  });
  describe("removeFromDb", () => {
    it("removes records matching constraints", () => {
      const db = createTestDb();
      const removed = removeFromDb({
        db,
        dbName: "users",
        constraints: { status: "inactive" },
      });
      expect(removed).toHaveLength(1);
      const remaining = queryDb({ db, dbName: "users", constraints: {} });
      expect(remaining).toHaveLength(1);
    });
    it("returns empty array if no matches", () => {
      const db = createTestDb();
      const removed = removeFromDb({
        db,
        dbName: "users",
        constraints: { status: "deleted" },
      });
      expect(removed).toHaveLength(0);
    });
    it("returns empty array if table does not exist", () => {
      const db = createTestDb();
      const removed = removeFromDb({
        db,
        dbName: "nonexistent",
      });
      expect(removed).toHaveLength(0);
    });
  });
  describe("listDatabases", () => {
    it("lists available databases in correct order", () => {
      const db = createTestDb();
      const names = listDatabases(db);
      expect(names).toContain("users");
      expect(names).toContain("accounts");
      expect(names).toContain("verification_history");
      expect(names.length).toBeGreaterThan(10);
    });
    it("returns databases in Python _load_databases order", () => {
      const db = createTestDb();
      const names = listDatabases(db);
      const userIdx = names.indexOf("users");
      const accountsIdx = names.indexOf("accounts");
      const verifyIdx = names.indexOf("verification_history");
      expect(userIdx).toBeLessThan(accountsIdx);
      expect(accountsIdx).toBeLessThan(verifyIdx);
    });
  });
  describe("queryDatabaseTool", () => {
    it("returns formatted query result header and records", () => {
      const db = createTestDb();
      const result = queryDatabaseTool(
        db,
        "users",
        JSON.stringify({ status: "active" })
      );
      expect(result).toContain("Found 1 record(s) in 'users':");
      expect(result).toContain("Record ID: usr1");
      expect(result).toContain("user_id: usr1");
    });
    it('returns "No records found" message', () => {
      const db = createTestDb();
      const result = queryDatabaseTool(
        db,
        "users",
        JSON.stringify({ status: "deleted" })
      );
      expect(result).toBe("No records found in 'users'.");
    });
    it("returns error for unknown database", () => {
      const db = createTestDb();
      const result = queryDatabaseTool(db, "nonexistent", "{}");
      expect(result).toContain("Error: Database 'nonexistent' not found");
      expect(result).toContain("Available:");
    });
    it("returns error for invalid JSON constraints", () => {
      const db = createTestDb();
      const result = queryDatabaseTool(db, "users", "invalid json");
      expect(result).toContain("Error: Invalid JSON");
    });
    it("lists available databases in error message", () => {
      const db = createTestDb();
      const result = queryDatabaseTool(db, "unknown", "{}");
      expect(result).toContain("users");
      expect(result).toContain("accounts");
    });
  });
  describe("State isolation", () => {
    it("modifications to one db do not affect another", () => {
      const db1 = createTestDb();
      const db2 = createTestDb();
      addToDb({
        db: db1,
        dbName: "users",
        recordId: "usr3",
        record: { user_id: "usr3" },
      });
      const results = queryDb({
        db: db2,
        dbName: "users",
        constraints: { user_id: "usr3" },
      });
      expect(results).toHaveLength(0);
    });
  });
});
