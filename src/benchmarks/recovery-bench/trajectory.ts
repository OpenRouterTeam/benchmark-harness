import { TaggedError } from "effect/Data";

import type { ModelMessage } from "../../harness/core";
import { MessageRole } from "../../harness/core";
import { Either } from "../../internal/either";
import { firstZodIssueMessage, parseSchema } from "../../internal/zod";
import type { AtifStep, AtifTrajectory } from "./schema";
import { AtifTrajectorySchema } from "./schema";

export interface ReplayCommand {
  readonly command: string;
  readonly keystrokes: string;
  readonly timeoutSec: number;
}

export interface ParsedTrajectory {
  readonly schemaVersion: string;
  readonly modelName: string | undefined;
  readonly commands: readonly ReplayCommand[];
  readonly messages: readonly ModelMessage[];
}

export class TrajectoryParseError extends TaggedError("TrajectoryParseError")<{
  readonly message: string;
}> {}

const CONTROL_PREFIX = "C-";

const DURATION_TIMEOUT_FACTOR = 10;

const DURATION_TIMEOUT_MARGIN_SEC = 5;

function stripTrailingNewlines(keystrokes: string): string {
  return keystrokes.replace(/[\r\n]+$/, "");
}

export function stepReplayCommands(step: AtifStep): readonly ReplayCommand[] {
  if (step.source !== "agent") {
    return [];
  }
  return (step.tool_calls ?? []).flatMap((toolCall) => {
    const keystrokes = toolCall.arguments?.keystrokes ?? "";
    if (keystrokes.length === 0) {
      return [];
    }
    const trimmed = stripTrailingNewlines(keystrokes);
    const command =
      trimmed.length > 0 && !trimmed.startsWith(CONTROL_PREFIX) ? trimmed : "";
    const duration = toolCall.arguments?.duration ?? 1;
    return [
      {
        command,
        keystrokes,
        timeoutSec:
          Math.trunc(duration * DURATION_TIMEOUT_FACTOR) +
          DURATION_TIMEOUT_MARGIN_SEC,
      },
    ];
  });
}

export function stepMessage(step: AtifStep): ModelMessage {
  const role = step.source === "agent" ? MessageRole.Assistant : step.source;
  return { role, content: step.message };
}

export function interruptedCommandIndices(
  commands: readonly ReplayCommand[]
): ReadonlySet<number> {
  const skip = new Set<number>();
  for (let i = 0; i < commands.length - 1; i += 1) {
    const current = commands[i];
    const next = commands[i + 1];
    if (
      current !== undefined &&
      next !== undefined &&
      current.command.length > 0 &&
      next.keystrokes.trim().startsWith(CONTROL_PREFIX)
    ) {
      skip.add(i);
    }
  }
  return skip;
}

export interface ReplayPlan {
  readonly commands: readonly string[];
  readonly skippedInterrupted: number;
  readonly totalExecutable: number;
}

export function planReplay(commands: readonly ReplayCommand[]): ReplayPlan {
  const skip = interruptedCommandIndices(commands);
  let skippedInterrupted = 0;
  let totalExecutable = 0;
  const toRun: string[] = [];
  commands.forEach((cmd, index) => {
    if (cmd.command.length === 0) {
      return;
    }
    totalExecutable += 1;
    if (skip.has(index)) {
      skippedInterrupted += 1;
      return;
    }
    toRun.push(cmd.command);
  });
  return { commands: toRun, skippedInterrupted, totalExecutable };
}

export function fromAtif(trajectory: AtifTrajectory): ParsedTrajectory {
  return {
    schemaVersion: trajectory.schema_version,
    modelName: trajectory.agent?.model_name,
    commands: trajectory.steps.flatMap((step) => stepReplayCommands(step)),
    messages: trajectory.steps.map((step) => stepMessage(step)),
  };
}

export function parseTrajectory(
  text: string
): Either.Either<ParsedTrajectory, TrajectoryParseError> {
  const json = Either.try((): unknown => JSON.parse(text));
  if (Either.isLeft(json)) {
    return Either.left(
      new TrajectoryParseError({
        message: `trajectory is not valid JSON: ${String(json.left)}`,
      })
    );
  }
  const parsed = parseSchema(AtifTrajectorySchema, json.right);
  if (Either.isLeft(parsed)) {
    return Either.left(
      new TrajectoryParseError({
        message: `trajectory failed ATIF validation: ${firstZodIssueMessage(parsed.left)}`,
      })
    );
  }
  return Either.right(fromAtif(parsed.right));
}
