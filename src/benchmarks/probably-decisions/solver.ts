import type { Effect } from "effect/Effect";
import {
  either,
  fail,
  gen,
  runPromise,
  succeed,
  tryPromise,
} from "effect/Effect";
import { isLeft, isRight } from "effect/Either";

import type {
  ModelMessage,
  ModelOutput,
  ModelUsage,
  TaskState,
  ToolDefinition,
} from "../../harness/core";
import { MessageRole, ModelError, SolverError } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import { withCallCacheSalt } from "../../runtime/response-cache";
import { readDecisionSampleMeta } from "./dataset";
import type { JudgeFn } from "./judge";
import { JUDGE_SYSTEM_PROMPT, judgePrompt } from "./judge";
import type { Value } from "./probably/language";
import type { Provider, Run, TraceEvent } from "./probably/runtime";
import { run as runProbably } from "./probably/runtime";
import type { ProbablyProgramId } from "./programs";
import { probablyProgramSource } from "./programs";
import type {
  DecisionSampleMeta,
  JudgeTrace,
  ProbablyMode,
  ProbablyRunMeta,
} from "./schema";
import { MAX_DOSSIER_CHARS } from "./schema";

export interface DecisionSolverOptions {
  readonly judge: JudgeFn;
  readonly mode: ProbablyMode;
  readonly program: ProbablyProgramId;
  readonly maxResearchSteps: number;
  readonly inference: GenerateConfig;
}

export const RESEARCH_SYSTEM_PROMPT = [
  "You are a Trust and Safety investigator preparing a case dossier for an adjudicator.",
  "You start with a one-sentence lead. Read the read-only evidence sections with the tools provided.",
  "Then call submit_dossier once with a factual dossier written in plain prose.",
  "State what the evidence shows about the actors involved, whether their behavior is shared or merely a shared static attribute, whether an API key appears compromised, what funding, spend, decline or key signals corroborate the lead, and what the evidence does not establish.",
  "Do not recommend a remedy and do not invent facts that are absent from the evidence.",
].join("\n");

export const RESEARCH_TOOLS: readonly ToolDefinition[] = [
  {
    type: "function",
    function: {
      name: "list_evidence_sections",
      description:
        "List the evidence sections available for this case and the number of fields in each.",
      parameters: {
        type: "object",
        properties: {},
        additionalProperties: false,
      },
    },
  },
  {
    type: "function",
    function: {
      name: "read_evidence_section",
      description: "Read one evidence section as JSON.",
      parameters: {
        type: "object",
        properties: { section: { type: "string" } },
        required: ["section"],
        additionalProperties: false,
      },
    },
  },
  {
    type: "function",
    function: {
      name: "submit_dossier",
      description:
        "Submit the final dossier for adjudication. Ends the investigation.",
      parameters: {
        type: "object",
        properties: { dossier: { type: "string" } },
        required: ["dossier"],
        additionalProperties: false,
      },
    },
  },
];

interface UsageAccumulator {
  inputTokens: number;
  outputTokens: number;
  totalTokens: number;
  reasoningTokens: number;
  totalCost: number;
  generationTimeMs: number;
}

function newUsage(): UsageAccumulator {
  return {
    inputTokens: 0,
    outputTokens: 0,
    totalTokens: 0,
    reasoningTokens: 0,
    totalCost: 0,
    generationTimeMs: 0,
  };
}

function addUsage(
  acc: UsageAccumulator,
  usage: ModelUsage | undefined,
  timeMs: number | undefined
): void {
  acc.inputTokens += usage?.inputTokens ?? 0;
  acc.outputTokens += usage?.outputTokens ?? 0;
  acc.totalTokens += usage?.totalTokens ?? 0;
  acc.reasoningTokens += usage?.reasoningTokens ?? 0;
  acc.totalCost += usage?.totalCost ?? 0;
  acc.generationTimeMs += timeMs ?? 0;
}

export function judgeTraces(trace: readonly TraceEvent[]): JudgeTrace[] {
  return trace.flatMap((event) =>
    event.kind === "judge"
      ? [
          {
            line: event.line,
            labels: [...event.detail.labels],
            probabilities: { ...event.detail.probabilities },
            chosen: event.detail.chosen,
            threshold: event.detail.threshold,
          },
        ]
      : []
  );
}

export function judgmentInput(meta: DecisionSampleMeta): string {
  return `${meta.dossier}\n\nEvidence:\n${JSON.stringify(meta.evidence)}`;
}

interface JudgmentOutcome {
  readonly run: Run | undefined;
  readonly failure: string | undefined;
  readonly messages: readonly ModelMessage[];
}

function runJudgment(
  judge: JudgeFn,
  opts: DecisionSolverOptions,
  input: string,
  saltPrefix: string,
  usage: UsageAccumulator
): Effect<JudgmentOutcome, ModelError> {
  return gen(function* () {
    let pending: ModelError | SolverError | undefined;
    let callIndex = 0;
    const messages: ModelMessage[] = [
      { role: MessageRole.System, content: JUDGE_SYSTEM_PROMPT },
    ];
    const provider: Provider = {
      write: () =>
        Promise.reject(new Error("write is not available in this benchmark.")),
      judge: async (value: Value, labels: readonly string[]) => {
        callIndex++;
        const salt = `${saltPrefix}-judge-${callIndex}`;
        messages.push({
          role: MessageRole.User,
          content: judgePrompt(value, labels),
        });
        const result = await runPromise(either(judge(salt, value, labels)));
        if (isLeft(result)) {
          pending = result.left;
          throw new Error(result.left.message);
        }
        messages.push({
          role: MessageRole.Assistant,
          content: result.right.completion,
        });
        addUsage(usage, result.right.usage, result.right.generationTimeMs);
        return result.right.probabilities;
      },
    };
    const attempt = yield* either(
      tryPromise({
        try: () =>
          runProbably(probablyProgramSource(opts.program), provider, {
            input,
            maxInputChars: MAX_DOSSIER_CHARS,
          }),
        catch: (error) =>
          error instanceof Error ? error.message : String(error),
      })
    );
    if (isRight(attempt)) {
      return { run: attempt.right, failure: undefined, messages };
    }
    if (pending instanceof ModelError) {
      return yield* fail(pending);
    }
    return {
      run: undefined,
      failure: pending?.message ?? attempt.left,
      messages,
    };
  });
}

interface ResearchOutcome {
  readonly messages: readonly ModelMessage[];
  readonly dossier: string | undefined;
  readonly sectionsRead: readonly string[];
  readonly steps: number;
}

function parseArguments(raw: string): Record<string, unknown> {
  const parsed = Either.try((): unknown => JSON.parse(raw));
  return Either.isRight(parsed) && isRecord(parsed.right) ? parsed.right : {};
}

function sectionSummary(evidence: DecisionSampleMeta["evidence"]): string {
  return JSON.stringify(
    Object.entries(evidence).map(([name, fields]) => ({
      section: name,
      fields: Object.keys(fields).length,
    }))
  );
}

function runResearch(
  model: ModelService,
  opts: DecisionSolverOptions,
  meta: DecisionSampleMeta,
  saltPrefix: string,
  usage: UsageAccumulator
): Effect<ResearchOutcome, ModelError> {
  return gen(function* () {
    const messages: ModelMessage[] = [
      { role: MessageRole.System, content: RESEARCH_SYSTEM_PROMPT },
      { role: MessageRole.User, content: `Lead: ${meta.lead}` },
    ];
    const sectionsRead = new Set<string>();
    const config: GenerateConfig = { ...opts.inference, tools: RESEARCH_TOOLS };
    let dossier: string | undefined;
    let steps = 0;
    while (dossier === undefined && steps < opts.maxResearchSteps) {
      steps++;
      const output = yield* withCallCacheSalt(
        `${saltPrefix}-research-${steps}`,
        model.generate(messages, config)
      );
      addUsage(usage, output.usage, output.generationTimeMs);
      messages.push(output.message);
      const toolCalls = output.message.toolCalls ?? [];
      if (toolCalls.length === 0) {
        messages.push({
          role: MessageRole.User,
          content:
            "Continue the investigation with the tools, or call submit_dossier.",
        });
        continue;
      }
      for (const call of toolCalls) {
        const args = parseArguments(call.function.arguments);
        const content = invokeResearchTool(
          call.function.name,
          args,
          meta,
          sectionsRead
        );
        if (
          call.function.name === "submit_dossier" &&
          typeof args["dossier"] === "string"
        ) {
          dossier = args["dossier"];
        }
        messages.push({ role: MessageRole.Tool, content, toolCallId: call.id });
      }
    }
    return {
      messages,
      dossier,
      sectionsRead: [...sectionsRead].toSorted(),
      steps,
    };
  });
}

export function invokeResearchTool(
  name: string,
  args: Readonly<Record<string, unknown>>,
  meta: DecisionSampleMeta,
  sectionsRead: Set<string>
): string {
  switch (name) {
    case "list_evidence_sections": {
      return sectionSummary(meta.evidence);
    }
    case "read_evidence_section": {
      const section = args["section"];
      const body =
        typeof section === "string" ? meta.evidence[section] : undefined;
      if (typeof section !== "string" || body === undefined) {
        return JSON.stringify({
          error: "unknown section",
          available: Object.keys(meta.evidence),
        });
      }
      sectionsRead.add(section);
      return JSON.stringify(body);
    }
    case "submit_dossier": {
      return typeof args["dossier"] === "string"
        ? JSON.stringify({ accepted: true })
        : JSON.stringify({ error: "dossier must be a string" });
    }
    default: {
      return JSON.stringify({ error: `unknown tool ${name}` });
    }
  }
}

function finalAction(run: Run | undefined): string | null {
  const last = run?.output.at(-1);
  return last === undefined ? null : last.trim();
}

function finishState(
  state: TaskState,
  messages: readonly ModelMessage[],
  runMeta: ProbablyRunMeta,
  usage: UsageAccumulator
): TaskState {
  const completion = runMeta.action ?? "";
  const output: ModelOutput = {
    completion,
    message: { role: MessageRole.Assistant, content: completion },
    usage: {
      inputTokens: usage.inputTokens,
      outputTokens: usage.outputTokens,
      totalTokens: usage.totalTokens,
      reasoningTokens: usage.reasoningTokens,
      totalCost: usage.totalCost,
    },
    generationTimeMs: usage.generationTimeMs,
  };
  return {
    ...state,
    sample: {
      ...state.sample,
      metadata: { ...state.sample.metadata, probablyRun: runMeta },
    },
    messages: [...messages, output.message],
    output,
    completed: true,
  };
}

export function makeDecisionSolver(
  model: ModelService,
  opts: DecisionSolverOptions
): SolverService {
  return (state) =>
    gen(function* () {
      const meta = readDecisionSampleMeta(state.sample.metadata);
      if (meta === undefined) {
        return yield* fail(
          new SolverError({
            message: `Sample ${state.sample.id} is missing decision metadata`,
          })
        );
      }
      const usage = newUsage();
      const saltPrefix = `${state.sample.id}-e${state.epoch ?? 0}`;
      const sectionsAvailable = Object.keys(meta.evidence).toSorted();
      if (opts.mode === "judgment") {
        const judged = yield* runJudgment(
          opts.judge,
          opts,
          judgmentInput(meta),
          saltPrefix,
          usage
        );
        return finishState(
          state,
          [...state.messages, ...judged.messages],
          {
            mode: "judgment",
            action: finalAction(judged.run),
            judges: judgeTraces(judged.run?.trace ?? []),
            researchDossier: null,
            sectionsAvailable,
            sectionsRead: sectionsAvailable,
            researchSteps: 0,
            failure: judged.failure ?? null,
          },
          usage
        );
      }
      const research = yield* runResearch(model, opts, meta, saltPrefix, usage);
      if (research.dossier === undefined) {
        return finishState(
          state,
          research.messages,
          {
            mode: "research",
            action: null,
            judges: [],
            researchDossier: null,
            sectionsAvailable,
            sectionsRead: research.sectionsRead,
            researchSteps: research.steps,
            failure: `No dossier submitted within ${opts.maxResearchSteps} steps`,
          },
          usage
        );
      }
      const judged = yield* runJudgment(
        opts.judge,
        opts,
        research.dossier,
        saltPrefix,
        usage
      );
      return yield* succeed(
        finishState(
          state,
          [...research.messages, ...judged.messages],
          {
            mode: "research",
            action: finalAction(judged.run),
            judges: judgeTraces(judged.run?.trace ?? []),
            researchDossier: research.dossier,
            sectionsAvailable,
            sectionsRead: research.sectionsRead,
            researchSteps: research.steps,
            failure: judged.failure ?? null,
          },
          usage
        )
      );
    });
}
