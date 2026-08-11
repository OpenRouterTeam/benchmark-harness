import { join } from "node:path";

import type { Effect } from "effect/Effect";
import {
  catchAll,
  gen,
  orElseSucceed,
  succeed,
  tryPromise,
} from "effect/Effect";

import type { ChatMessage, ModelUsage } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { SolverService } from "../../harness/solver";
import { unknownErrorToString } from "../../internal/errors";
import { eLog, wLog } from "../../internal/log";
import { parseReward } from "../harbor/reward";
import type {
  SandboxSessionFactory,
  SandboxSessionInstance,
} from "../terminal-bench/sandbox";
import {
  agentDxTasksDir,
  isSafeAgentDxTaskId,
  readAgentDxMeta,
} from "./dataset";
import {
  COLLECT_EVIDENCE_SCRIPT,
  parseAlignmentEvidence,
  parseDiscoverabilityEvidence,
} from "./discoverability";
import {
  buildClaudeCodeImageSteps,
  buildClaudeCodeRunScript,
  buildOpencodeImageSteps,
  buildOpencodeRunScript,
  parseClaudeCodeUsage,
  parseOpencodeUsage,
  usesSkills,
} from "./harness";
import {
  DEFAULT_AGENT_DX_JUDGE_MODEL,
  buildJudgePrompt,
  buildJudgeResponseFormat,
  judgeTextFromResponse,
  parseJudgeVerdict,
  parseSubcheckSummary,
} from "./quality";
import type { VerifierVerdict } from "./result-row-metrics";
import { parseVerifierVerdict } from "./result-row-metrics";
import type {
  AgentDxHarness,
  AgentDxProfile,
  AgentDxSandboxKeyMode,
} from "./schema";
import {
  AGENT_DX_DOCS_SOURCE_PATTERN,
  AGENT_DX_OPENCODE_PACKAGE_PATTERN,
  AGENT_DX_SKILLS_SOURCE_PATTERN,
  REMOTE_AGENT_STDERR_LOG,
} from "./schema";

export interface AgentDxSolverOpts {
  readonly model: string;
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly profile: AgentDxProfile;
  readonly opencodePackage: string;
  readonly skillsSource: string;
  readonly docsSource: string;
  readonly docsAddendum?: string;
  readonly mcpAddendum?: string;
  readonly judgeModel?: string | null;
  readonly sandboxKey: AgentDxSandboxKeyMode;
}

const REMOTE_REWARD_PATH = "/logs/verifier/reward.txt" as const;
const REMOTE_VERDICT_PATH = "/logs/verifier/verdict.json" as const;

interface HarnessSpec {
  readonly validate: (opts: AgentDxSolverOpts) => string | undefined;
  readonly imageSteps: (opts: AgentDxSolverOpts) => string[];
  readonly runScript: (opts: AgentDxSolverOpts) => string;
  readonly parseUsage: (eventStream: string) => ModelUsage | undefined;
}

const OPENCODE_SPEC: HarnessSpec = {
  validate: (opts) =>
    AGENT_DX_OPENCODE_PACKAGE_PATTERN.test(opts.opencodePackage)
      ? undefined
      : `benchmark config: invalid opencodePackage "${opts.opencodePackage}": must be an npm package name with optional @version`,
  imageSteps: (opts) =>
    buildOpencodeImageSteps({
      opencodePackage: opts.opencodePackage,
      profile: opts.profile,
      skillsSource: opts.skillsSource,
      docsSource: opts.docsSource,
    }),
  runScript: (opts) =>
    buildOpencodeRunScript({
      profile: opts.profile,
      sandboxKey: opts.sandboxKey,
      ...(opts.docsAddendum !== undefined && {
        docsAddendum: opts.docsAddendum,
      }),
      ...(opts.mcpAddendum !== undefined && { mcpAddendum: opts.mcpAddendum }),
    }),
  parseUsage: parseOpencodeUsage,
};

const CLAUDE_CODE_SPEC: HarnessSpec = {
  validate: () => undefined,
  imageSteps: (opts) =>
    buildClaudeCodeImageSteps({
      profile: opts.profile,
      skillsSource: opts.skillsSource,
      docsSource: opts.docsSource,
    }),
  runScript: (opts) =>
    buildClaudeCodeRunScript({
      profile: opts.profile,
      sandboxKey: opts.sandboxKey,
      ...(opts.docsAddendum !== undefined && {
        docsAddendum: opts.docsAddendum,
      }),
      ...(opts.mcpAddendum !== undefined && { mcpAddendum: opts.mcpAddendum }),
    }),
  parseUsage: parseClaudeCodeUsage,
};

export function harnessSolver(
  sessionFactory: SandboxSessionFactory,
  opts: AgentDxSolverOpts,
  harness: AgentDxHarness
): SolverService {
  return specSolver(sessionFactory, opts, specFor(harness));
}

function specFor(harness: AgentDxHarness): HarnessSpec {
  switch (harness) {
    case "opencode": {
      return OPENCODE_SPEC;
    }
    case "claude-code": {
      return CLAUDE_CODE_SPEC;
    }
    default: {
      return harness satisfies never;
    }
  }
}

function specSolver(
  sessionFactory: SandboxSessionFactory,
  opts: AgentDxSolverOpts,
  spec: HarnessSpec
): SolverService {
  const { profile, skillsSource, docsSource } = opts;

  return (state) =>
    gen(function* () {
      const meta = readAgentDxMeta(state.sample.metadata);
      if (meta === undefined) {
        return yield* new SolverError({
          message: `benchmark config: agent-dx solver received a sample without agent-dx metadata (id=${state.sample.id})`,
        });
      }

      if (
        usesSkills(profile) &&
        !AGENT_DX_SKILLS_SOURCE_PATTERN.test(skillsSource)
      ) {
        return yield* new SolverError({
          message: `benchmark config: invalid skillsSource "${skillsSource}": must be an https git URL with optional #ref`,
        });
      }

      if (
        profile === "docs" &&
        !AGENT_DX_DOCS_SOURCE_PATTERN.test(docsSource)
      ) {
        return yield* new SolverError({
          message: `benchmark config: invalid docsSource "${docsSource}": must be a plain https URL`,
        });
      }

      const harnessConfigError = spec.validate(opts);
      if (harnessConfigError !== undefined) {
        return yield* new SolverError({ message: harnessConfigError });
      }

      const presetSlugBase = meta.taskEnv["ADX_PRESET_SLUG"];
      if (
        presetSlugBase !== undefined &&
        !/^[A-Za-z0-9_-]+$/.test(presetSlugBase)
      ) {
        return yield* new SolverError({
          message: `benchmark config: invalid ADX_PRESET_SLUG "${presetSlugBase}": must match [A-Za-z0-9_-]+`,
        });
      }

      if (!isSafeAgentDxTaskId(meta.taskId)) {
        return yield* new SolverError({
          message: `benchmark config: invalid agent-dx task id "${meta.taskId}" in sample metadata`,
        });
      }

      const taskDir = join(agentDxTasksDir(), meta.taskId);
      const session = yield* sessionFactory.create({
        imageTag: meta.dockerImage,
        maxAgentTimeoutSec: meta.maxAgentTimeoutSec,
        maxTestTimeoutSec: meta.maxTestTimeoutSec,
        testDir: join(taskDir, "tests"),
        testScript: join(taskDir, "tests", "test.sh"),
        instructionPath: join(taskDir, "instruction.md"),
        imageBuildSteps: spec.imageSteps(opts),
        sandboxBufferSec: 600,
      });

      try {
        const agentEnv: Record<string, string> = {
          ...meta.taskEnv,
          ...(meta.taskEnv["ADX_PRESET_SLUG"] !== undefined && {
            ADX_PRESET_SLUG: `${meta.taskEnv["ADX_PRESET_SLUG"]}-${crypto.randomUUID().slice(0, 8)}`,
          }),
          ...(opts.sandboxKey === "absent"
            ? { ADX_HARNESS_KEY: opts.apiKey }
            : { OPENROUTER_API_KEY: opts.apiKey }),
          ADX_MODEL: opts.model,
          ...(opts.baseUrl !== undefined && {
            ...(opts.sandboxKey === "provided" && {
              OPENROUTER_BASE_URL: opts.baseUrl,
            }),
            ADX_OPENROUTER_ORIGIN: normalizeOpenRouterOrigin(opts.baseUrl),
          }),
        };
        const agentRun = yield* session.exec(
          ["bash", "-c", spec.runScript(opts)],
          agentEnv,
          meta.maxAgentTimeoutSec * 1000 + 30_000
        );
        const secrets = [opts.apiKey];
        const eventStream = redactKeyMaterial(agentRun.stdout, secrets);
        const agentUsage = spec.parseUsage(eventStream);
        const agentStderr =
          agentRun.exitCode === 0
            ? ""
            : (yield* session
                .exec(["cat", REMOTE_AGENT_STDERR_LOG], {}, 10_000)
                .pipe(
                  catchAll((error) => {
                    wLog("agent-dx: agent stderr read-back failed", {
                      error_message: error.message,
                    });
                    return succeed({ stdout: "", stderr: "", exitCode: 1 });
                  })
                )).stdout;
        const agentExitDetail =
          agentRun.exitCode === 0
            ? ""
            : prefixLines(
                `agent harness exited ${agentRun.exitCode}. output: ${eventStream}\nstderr: ${redactKeyMaterial(agentStderr, secrets)}`,
                "[agent] "
              );

        const verifierEnv: Record<string, string> = {
          ...agentEnv,
          OPENROUTER_API_KEY: opts.apiKey,
        };
        const {
          reward,
          output,
          verdict: verifierVerdict,
        } = yield* runVerifier(session, verifierEnv, meta.maxTestTimeoutSec);
        const testOutput = redactKeyMaterial(
          agentExitDetail ? `${agentExitDetail}\n\n${output}` : output,
          secrets
        );
        const subchecks = parseSubcheckSummary(
          redactKeyMaterial(output, secrets)
        );

        const evidenceRun = yield* session
          .exec(["bash", "-c", COLLECT_EVIDENCE_SCRIPT], {}, 60_000)
          .pipe(
            catchAll((error) => {
              wLog("agent-dx: discoverability scan failed", {
                error_message: error.message,
              });
              return succeed({ stdout: "", stderr: "", exitCode: 1 });
            })
          );
        const discoverability = parseDiscoverabilityEvidence(
          evidenceRun.stdout
        );
        const alignmentEvidence = parseAlignmentEvidence(evidenceRun.stdout);

        const judgeModel =
          opts.judgeModel === undefined
            ? DEFAULT_AGENT_DX_JUDGE_MODEL
            : opts.judgeModel;
        const verdict =
          judgeModel === null
            ? undefined
            : yield* judgeWorkspaceQuality({
                session,
                instruction: state.sample.input,
                judgeModel,
                apiKey: opts.apiKey,
                baseUrl: opts.baseUrl,
                secrets,
                alignmentCriteria: meta.alignmentCriteria,
              });

        const messages: ChatMessage[] = [
          { role: MessageRole.User, content: state.sample.input },
          { role: MessageRole.Assistant, content: eventStream },
        ];

        return {
          sample: {
            ...state.sample,
            metadata: {
              ...state.sample.metadata,
              reward,
              testOutput,
              ...(reward === 0 &&
                verifierVerdict !== undefined && {
                  verdictKind: verifierVerdict.kind,
                }),
              ...(subchecks !== undefined && {
                subchecksPassed: subchecks.passed,
                subchecksTotal: subchecks.total,
              }),
              ...(verdict !== undefined && {
                quality: verdict.quality,
                qualityCriteria: verdict.criteria,
                ...(verdict.alignment !== undefined && {
                  alignment: verdict.alignment,
                  alignmentVerdicts: verdict.alignmentCriteria,
                }),
              }),
              ...(discoverability !== undefined && {
                openrouterChosen: discoverability.openrouterChosen,
                openrouterEvidence: discoverability.evidence,
              }),
              ...(alignmentEvidence !== undefined && { alignmentEvidence }),
            },
          },
          messages,
          output: {
            completion: eventStream,
            message: { role: MessageRole.Assistant, content: eventStream },
            usage: agentUsage ?? {
              inputTokens: 0,
              outputTokens: 0,
              totalTokens: 0,
              reasoningTokens: 0,
              totalCost: 0,
            },
            generationTimeMs: 0,
          },
          completed: true,
        };
      } finally {
        yield* session.destroy();
      }
    });
}

export function normalizeOpenRouterOrigin(baseUrl: string): string {
  return baseUrl.replace(/\/+$/, "").replace(/\/api\/v1$/, "");
}

export function redactKeyMaterial(
  text: string,
  secrets: readonly string[] = []
): string {
  const withoutPrefixed = text.replaceAll(
    /sk-or-[A-Za-z0-9_-]+/g,
    "sk-or-<redacted>"
  );
  let redacted = withoutPrefixed;
  for (const secret of secrets) {
    if (secret !== "") {
      redacted = redacted.replaceAll(secret, "<redacted>");
    }
  }
  return redacted;
}

export function prefixLines(text: string, prefix: string): string {
  return text
    .split("\n")
    .map((line) => `${prefix}${line}`)
    .join("\n");
}

function runVerifier(
  session: SandboxSessionInstance,
  env: Readonly<Record<string, string>>,
  maxTestTimeoutSec: number
): Effect<
  { reward: number; output: string; verdict: VerifierVerdict | undefined },
  SolverError
> {
  return gen(function* () {
    const timeoutMs = Math.round(maxTestTimeoutSec * 1000) + 5000;
    const run = yield* session.exec(
      ["bash", "-c", "mkdir -p /logs/verifier && bash /tests/test.sh"],
      env,
      timeoutMs
    );
    const rewardRead = yield* session.exec(
      ["cat", REMOTE_REWARD_PATH],
      {},
      10_000
    );
    const verdictRead = yield* session
      .exec(
        ["bash", "-c", `cat ${REMOTE_VERDICT_PATH} 2>/dev/null || true`],
        {},
        10_000
      )
      .pipe(
        catchAll((error) => {
          wLog("agent-dx: verifier verdict read-back failed", {
            error_message: error.message,
          });
          return succeed({ stdout: "", stderr: "", exitCode: 1 });
        })
      );
    return {
      reward: parseReward(rewardRead.stdout),
      output: `${run.stdout}\n${run.stderr}`.trim(),
      verdict: parseVerifierVerdict(verdictRead.stdout),
    };
  });
}

const COLLECT_WORKSPACE_SCRIPT = [
  "find /app -type f \\( -name '*.ts' -o -name '*.js' -o -name '*.mjs' -o -name '*.cjs' -o -name '*.py' -o -name '*.json' -o -name '*.md' \\)",
  "-not -path '*/node_modules/*' -not -name 'package-lock.json'",
  "-not -path '/app/AGENTS.md' -not -path '/app/opencode.json'",
  "-not -path '/app/CLAUDE.md' -not -path '/app/.mcp.json' -not -path '/app/.claude/*' | sort | head -30 |",
  "while IFS= read -r f; do printf '=== %s ===\\n' \"$f\"; head -c 16000 \"$f\"; printf '\\n'; done",
].join(" ");

function judgeWorkspaceQuality(input: {
  readonly session: SandboxSessionInstance;
  readonly instruction: string;
  readonly judgeModel: string;
  readonly apiKey: string;
  readonly baseUrl: string | undefined;
  readonly secrets: readonly string[];
  readonly alignmentCriteria: readonly string[];
}): Effect<ReturnType<typeof parseJudgeVerdict>, never> {
  return gen(function* () {
    const collected = yield* input.session
      .exec(["bash", "-c", COLLECT_WORKSPACE_SCRIPT], {}, 60_000)
      .pipe(
        catchAll((error) => {
          wLog("agent-dx: judge workspace collection failed", {
            error_message: error.message,
          });
          return succeed({ stdout: "", stderr: "", exitCode: 1 });
        })
      );
    const workspace = redactKeyMaterial(collected.stdout, input.secrets).trim();
    if (workspace === "") {
      return undefined;
    }
    return yield* tryPromise({
      try: () =>
        callJudge({
          judgeModel: input.judgeModel,
          apiKey: input.apiKey,
          baseUrl: input.baseUrl,
          instruction: input.instruction,
          workspace,
          alignmentCriteria: input.alignmentCriteria,
        }),
      catch: (cause) => {
        eLog("agent-dx: judge call failed", {
          judge_model: input.judgeModel,
          error_message: unknownErrorToString(cause),
        });
        return new SolverError({ message: "judge call failed" });
      },
    }).pipe(orElseSucceed(() => undefined));
  });
}

async function callJudge(input: {
  readonly judgeModel: string;
  readonly apiKey: string;
  readonly baseUrl: string | undefined;
  readonly instruction: string;
  readonly workspace: string;
  readonly alignmentCriteria: readonly string[];
}): Promise<ReturnType<typeof parseJudgeVerdict>> {
  const origin = normalizeOpenRouterOrigin(
    input.baseUrl ?? "https://openrouter.ai"
  );
  const response = await fetch(`${origin}/api/v1/chat/completions`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${input.apiKey}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      model: input.judgeModel,
      messages: [
        {
          role: "user",
          content: buildJudgePrompt(
            input.instruction,
            input.workspace,
            input.alignmentCriteria
          ),
        },
      ],
      temperature: 0,
      max_tokens: 2000,
      response_format: buildJudgeResponseFormat(input.alignmentCriteria),
    }),
    signal: AbortSignal.timeout(90_000),
  });
  if (!response.ok) {
    await response.body?.cancel();
    return undefined;
  }
  const text = judgeTextFromResponse(await response.json());
  return text === undefined
    ? undefined
    : parseJudgeVerdict(text, input.alignmentCriteria);
}
