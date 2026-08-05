import { describe, expect, it } from "bun:test";

import { selectUserToolDefinitions, TerminationReason } from "./solver";
describe("bankingSolver", () => {
  it("exports TerminationReason with required values", () => {
    expect(TerminationReason.UserStop).toBe("USER_STOP");
    expect(TerminationReason.MaxSteps).toBe("MAX_STEPS");
  });
  it("grants only the user tools declared by the task", () => {
    const definitions = selectUserToolDefinitions([
      "apply_for_credit_card",
      "call_discoverable_user_tool",
    ]);
    expect(definitions.map((definition) => definition.function.name)).toEqual([
      "apply_for_credit_card",
      "call_discoverable_user_tool",
    ]);
  });
});
