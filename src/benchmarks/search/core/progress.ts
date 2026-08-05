import type { OutputItems, StreamEvents } from "@openrouter/sdk/models";

import type { AgentStepEvent } from "../../../harness/progress";

const WEB_SEARCH_ITEM_TYPES: ReadonlySet<string> = new Set([
  "web_search_call",
  "openrouter:web_search",
]);

export function makeSearchProgressTracker(): (
  event: StreamEvents
) => AgentStepEvent | undefined {
  let step = 0;
  let toolCallIndex = 0;
  return (event) => {
    switch (event.type) {
      case "response.output_item.added": {
        step = event.outputIndex;
        return { type: "turn", step, toolCallIndex };
      }
      case "response.output_item.done": {
        const command = webSearchCommand(event.item);
        if (command === undefined) {
          return undefined;
        }
        step = event.outputIndex;
        toolCallIndex += 1;
        return { type: "tool-call", step, toolCallIndex, command };
      }
      default: {
        return undefined;
      }
    }
  };
}

function webSearchCommand(item: OutputItems): string | undefined {
  if (typeof item.type !== "string" || !WEB_SEARCH_ITEM_TYPES.has(item.type)) {
    return undefined;
  }
  if (
    "action" in item &&
    item.action !== undefined &&
    item.action.type === "search"
  ) {
    return item.action.query;
  }
  return item.type;
}
