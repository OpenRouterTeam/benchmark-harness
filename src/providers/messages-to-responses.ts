import type {
  ModelMessage,
  ContentPart,
  ModelOutput,
  ToolDefinition,
} from "../harness/core";
import { MessageRole } from "../harness/core";
import type {
  ResponsesFunctionTool,
  ResponsesInputItem,
  ResponsesTurn,
} from "./responses-model";

export function messagesToResponses(
  messages: readonly ModelMessage[]
): readonly ResponsesInputItem[] {
  return messages.flatMap(messageToResponses);
}

function messageToResponses(message: ModelMessage): ResponsesInputItem[] {
  switch (message.role) {
    case MessageRole.System:
    case MessageRole.User: {
      return [
        {
          type: "message",
          role: message.role,
          content:
            message.role === MessageRole.User &&
            message.contentParts !== undefined
              ? message.contentParts.map(contentPartToResponses)
              : message.content,
        },
      ];
    }
    case MessageRole.Assistant: {
      if (message.responseItems !== undefined) {
        return [...message.responseItems];
      }
      const items: ResponsesInputItem[] = [];
      if (message.content.length > 0) {
        items.push({
          type: "message",
          role: "assistant",
          content: message.content,
        });
      }
      for (const toolCall of message.toolCalls ?? []) {
        items.push({
          type: "function_call",
          call_id: toolCall.id,
          name: toolCall.function.name,
          arguments: toolCall.function.arguments,
        });
      }
      return items;
    }
    case MessageRole.Tool: {
      return [
        {
          type: "function_call_output",
          call_id: message.toolCallId ?? "",
          output: message.content,
        },
      ];
    }
    default: {
      return message.role satisfies never;
    }
  }
}

function contentPartToResponses(part: ContentPart): ResponsesInputItem {
  switch (part.type) {
    case "text": {
      return { type: "input_text", text: part.text };
    }
    case "image_url": {
      return {
        type: "input_image",
        image_url: part.imageUrl.url,
        detail: part.imageUrl.detail ?? "auto",
      };
    }
    case "video_url": {
      return {
        type: "input_video",
        video_url: part.videoUrl.url,
        ...(part.videoUrl.processing !== undefined && {
          processing: part.videoUrl.processing,
        }),
      };
    }
    default: {
      return part satisfies never;
    }
  }
}

export function toolDefinitionToResponses(
  tool: ToolDefinition
): ResponsesFunctionTool {
  return {
    type: "function",
    name: tool.function.name,
    ...(tool.function.description !== undefined && {
      description: tool.function.description,
    }),
    parameters: tool.function.parameters ?? {},
    ...(tool.function.strict !== undefined && {
      strict: tool.function.strict,
    }),
  };
}

export function responsesTurnToModelOutput(turn: ResponsesTurn): ModelOutput {
  return {
    completion: turn.text,
    message: {
      role: MessageRole.Assistant,
      content: turn.text,
      ...(turn.functionCalls.length > 0 && {
        toolCalls: turn.functionCalls.map((call) => ({
          id: call.callId,
          type: "function" as const,
          function: {
            name: call.name,
            arguments: call.arguments,
          },
        })),
      }),
      responseItems: turn.outputItems,
    },
    ...(turn.usage !== undefined && { usage: turn.usage }),
    generationTimeMs: turn.generationTimeMs,
  };
}
