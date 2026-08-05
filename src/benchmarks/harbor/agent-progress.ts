import type { AgentStepEvent } from "../../harness/progress";

export function makeHarborStreamTracker(
  step: number,
  toolCallIndex: number
): (event: Record<string, unknown>) => AgentStepEvent | undefined {
  return (event) =>
    event["type"] === "response.output_item.added"
      ? { type: "turn", step, toolCallIndex }
      : undefined;
}
