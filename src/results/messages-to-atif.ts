import type {
  Citation,
  ContentPart,
  ModelMessage,
  ScorerTrajectory,
  ToolCall,
} from "../harness/core";
import { MessageRole } from "../harness/core";
import { Either } from "../internal/either";
import { definedValues, isRecord } from "../internal/guards";
import { firstZodIssueMessage, parseSchema } from "../internal/zod";
import type {
  AtifContentPart,
  AtifImageSource,
  AtifStep,
  AtifTrajectory,
} from "./atif-schema";
import { ATIF_SCHEMA_VERSION, AtifTrajectorySchema } from "./atif-schema";
import { RESULT_FORMAT_VERSION, RESULT_WRITER } from "./parquet-schema";

interface AtifStepDraft {
  readonly step_id: number;
  readonly source: AtifStep["source"];
  readonly message: AtifStep["message"];
  readonly model_name?: string;
  readonly reasoning_content?: string;
  readonly tool_calls?: AtifStep["tool_calls"];
  readonly observation?: AtifStep["observation"];
  readonly extra?: Record<string, unknown>;
}

const IMAGE_MEDIA_TYPES: Readonly<
  Record<string, AtifImageSource["media_type"]>
> = {
  gif: "image/gif",
  jpeg: "image/jpeg",
  jpg: "image/jpeg",
  png: "image/png",
  webp: "image/webp",
};

export function messagesToAtif(input: {
  readonly messages: readonly ModelMessage[];
  readonly model: string;
  readonly scorerTrajectory?: ScorerTrajectory;
}): AtifTrajectory | null {
  if (input.messages.length === 0) {
    return null;
  }

  const steps: AtifStepDraft[] = [];
  for (const message of input.messages) {
    if (message.role === MessageRole.Tool) {
      appendToolObservation(steps, message);
      continue;
    }
    steps.push(messageToStep(message, steps.length + 1));
  }
  const trajectory = definedValues({
    schema_version: ATIF_SCHEMA_VERSION,
    agent: {
      name: RESULT_WRITER,
      version: String(RESULT_FORMAT_VERSION),
      model_name: input.model,
    },
    steps,
    extra:
      input.scorerTrajectory === undefined
        ? undefined
        : { scorer_trajectory: input.scorerTrajectory },
  });
  const parsed = parseSchema(AtifTrajectorySchema, trajectory);
  if (Either.isLeft(parsed)) {
    throw new Error(
      `Invalid ATIF trajectory: ${firstZodIssueMessage(parsed.left)}`
    );
  }
  return parsed.right;
}

export function contentPartToPojo(part: ContentPart): Record<string, unknown> {
  switch (part.type) {
    case "image_url": {
      return {
        type: "image_url",
        image_url: definedValues({
          url: part.imageUrl.url,
          detail: part.imageUrl.detail,
        }),
      };
    }
    case "text": {
      return { type: "text", text: part.text };
    }
    case "video_url": {
      return {
        type: "video_url",
        video_url: definedValues({
          url: part.videoUrl.url,
          processing: part.videoUrl.processing,
        }),
      };
    }
    default: {
      part satisfies never;
      throw new Error(`Unhandled content part type: ${part}`);
    }
  }
}

export function citationToPojo(citation: Citation): Record<string, unknown> {
  return {
    url: citation.url,
    title: citation.title,
    start_index: citation.startIndex,
    end_index: citation.endIndex,
  };
}

function messageToStep(message: ModelMessage, stepId: number): AtifStepDraft {
  const messageValue = messageToAtifMessage(message);
  const extra = definedValues({
    ...(messageValue.unsupportedContentParts.length > 0 && {
      unsupported_content_parts: messageValue.unsupportedContentParts,
    }),
    ...(message.citations !== undefined &&
      message.citations.length > 0 && {
        citations: message.citations.map(citationToPojo),
      }),
  });
  const common: Omit<AtifStepDraft, "source"> = {
    step_id: stepId,
    message: messageValue.message,
    ...(Object.keys(extra).length > 0 && { extra }),
  };
  switch (message.role) {
    case MessageRole.System: {
      return { ...common, source: "system" };
    }
    case MessageRole.User: {
      return { ...common, source: "user" };
    }
    case MessageRole.Assistant: {
      return {
        ...common,
        source: "agent",
        ...(message.model === undefined ? {} : { model_name: message.model }),
        ...(message.reasoning === undefined
          ? {}
          : { reasoning_content: message.reasoning }),
        ...(message.toolCalls === undefined || message.toolCalls.length === 0
          ? {}
          : { tool_calls: message.toolCalls.map(toolCallToAtif) }),
      } satisfies AtifStepDraft;
    }
    case MessageRole.Tool: {
      throw new Error("Tool messages are observations");
    }
    default: {
      message.role satisfies never;
      throw new Error(`Unhandled message role: ${message.role}`);
    }
  }
}

function messageToAtifMessage(message: ModelMessage): {
  readonly message: string | AtifContentPart[];
  readonly unsupportedContentParts: readonly Record<string, unknown>[];
} {
  if (message.contentParts === undefined || message.contentParts.length === 0) {
    return { message: message.content, unsupportedContentParts: [] };
  }
  const supported: AtifContentPart[] = [];
  const unsupported: Record<string, unknown>[] = [];
  for (const part of message.contentParts) {
    const converted = contentPartToAtif(part);
    if (converted === undefined) {
      unsupported.push(contentPartToPojo(part));
    } else {
      supported.push(converted);
    }
  }
  return {
    message: supported.length > 0 ? supported : message.content,
    unsupportedContentParts: unsupported,
  };
}

function contentPartToAtif(part: ContentPart): AtifContentPart | undefined {
  switch (part.type) {
    case "text": {
      return { type: "text", text: part.text };
    }
    case "image_url": {
      const mediaType = imageMediaType(part.imageUrl.url);
      return mediaType === undefined
        ? undefined
        : {
            type: "image",
            source: { media_type: mediaType, path: part.imageUrl.url },
          };
    }
    case "video_url": {
      return undefined;
    }
    default: {
      part satisfies never;
      throw new Error(`Unhandled content part type: ${part}`);
    }
  }
}

function imageMediaType(
  url: string
): AtifImageSource["media_type"] | undefined {
  const label =
    /^data:image\/([a-z]+)[;,]/i.exec(url)?.[1] ??
    /(?:^|[/?])[^/?#]+\.([a-z]+)(?:[?#]|$)/i.exec(url)?.[1];
  return label === undefined
    ? undefined
    : IMAGE_MEDIA_TYPES[label.toLowerCase()];
}

function toolCallToAtif(
  toolCall: ToolCall
): NonNullable<AtifStep["tool_calls"]>[number] {
  return {
    tool_call_id: toolCall.id,
    function_name: toolCall.function.name,
    arguments: parseArguments(toolCall.function.arguments),
  };
}

function parseArguments(raw: string): Record<string, unknown> {
  try {
    const parsed: unknown = JSON.parse(raw);
    return isRecord(parsed) ? parsed : { raw };
  } catch {
    return { raw };
  }
}

function appendToolObservation(
  steps: AtifStepDraft[],
  message: ModelMessage
): void {
  const matchedIndex =
    message.toolCallId === undefined
      ? -1
      : steps.findLastIndex((step) =>
          (step.tool_calls ?? []).some(
            (toolCall) => toolCall.tool_call_id === message.toolCallId
          )
        );
  const observationIndex =
    matchedIndex !== -1 ? matchedIndex : fallbackObservationIndex(steps);
  if (observationIndex === -1) {
    return;
  }
  const agentStep = steps[observationIndex]!;
  const toolCallId = message.toolCallId;
  const hasMatchingToolCall = matchedIndex !== -1;
  const messageValue = messageToAtifMessage(message);
  const extra = definedValues({
    ...(!hasMatchingToolCall &&
      toolCallId !== undefined && {
        tool_call_id: toolCallId,
      }),
    ...(messageValue.unsupportedContentParts.length > 0 && {
      unsupported_content_parts: messageValue.unsupportedContentParts,
    }),
  });
  const result = definedValues({
    ...(hasMatchingToolCall && { source_call_id: toolCallId }),
    content: messageValue.message,
    ...(Object.keys(extra).length > 0 && { extra }),
  });
  const updatedStep: AtifStepDraft = {
    ...agentStep,
    observation: {
      results: [...(agentStep.observation?.results ?? []), result],
    },
  };
  steps[observationIndex] = updatedStep;
}

function fallbackObservationIndex(steps: readonly AtifStepDraft[]): number {
  const agentIndex = steps.findLastIndex((step) => step.source === "agent");
  return agentIndex !== -1 ? agentIndex : steps.length - 1;
}
