import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseArgs } from "node:util";

import {
  buildRecoveryInstruction,
  formatMessagesAsText,
  utf8ByteLength,
} from "../src/benchmarks/recovery-bench/prompts";
import {
  RECOVERY_BENCH_INITIAL_RUN,
  RECOVERY_BENCH_SOURCE_COMMIT,
} from "../src/benchmarks/recovery-bench/tasks-source";
import { parseTrajectory } from "../src/benchmarks/recovery-bench/trajectory";
import { Either } from "../src/internal/either";
import { parseSchema, z } from "../src/internal/zod";

const ResultSchema = z.object({
  task_name: z.string().min(1),
  verifier_result: z
    .object({
      rewards: z.object({ reward: z.number().nullable() }).nullable(),
    })
    .nullable(),
});

const DEFAULT_OUT = join(
  import.meta.dir,
  "..",
  "src",
  "benchmarks",
  "recovery-bench",
  "recovery-bench-manifest.json"
);

function sha256Hex(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function main(): void {
  const { values } = parseArgs({
    options: {
      checkout: { type: "string" },
      out: { type: "string", default: DEFAULT_OUT },
    },
  });
  const checkout = values.checkout;
  if (checkout === undefined) {
    throw new Error(
      "--checkout <path to a letta-ai/recovery-bench clone with LFS objects pulled> is required"
    );
  }
  const runDir = join(checkout, "runs", RECOVERY_BENCH_INITIAL_RUN);
  const trials = readdirSync(runDir)
    .filter((entry) => statSync(join(runDir, entry)).isDirectory())
    .sort()
    .flatMap((trialDir) => {
      const resultPath = join(runDir, trialDir, "result.json");
      const resultBytes = (() => {
        try {
          return readFileSync(resultPath);
        } catch {
          return undefined;
        }
      })();
      if (resultBytes === undefined) {
        return [];
      }
      const result = parseSchema(
        ResultSchema,
        JSON.parse(resultBytes.toString("utf8"))
      );
      if (Either.isLeft(result)) {
        throw new Error(`${resultPath}: ${result.left.message}`);
      }
      const reward = result.right.verifier_result?.rewards?.reward ?? 0;
      if (reward > 0) {
        return [];
      }
      const trajectoryPath = join(runDir, trialDir, "agent", "trajectory.json");
      const trajectoryBytes = readFileSync(trajectoryPath);
      const parsed = parseTrajectory(trajectoryBytes.toString("utf8"));
      if (Either.isLeft(parsed)) {
        throw new Error(`${trajectoryPath}: ${parsed.left.message}`);
      }
      const fullContextBytes = utf8ByteLength(
        buildRecoveryInstruction(
          "(task instruction)",
          formatMessagesAsText(parsed.right.messages)
        )
      );
      return [
        {
          taskId: result.right.task_name,
          trialDir,
          trajectorySha256: sha256Hex(trajectoryBytes),
          trajectoryBytes: trajectoryBytes.byteLength,
          fullContextBytes,
          replayCommands: parsed.right.commands.length,
        },
      ];
    });
  const manifest = {
    sourceCommit: RECOVERY_BENCH_SOURCE_COMMIT,
    run: RECOVERY_BENCH_INITIAL_RUN,
    trials,
  };
  writeFileSync(values.out, `${JSON.stringify(manifest, null, 2)}\n`);
  process.stdout.write(`wrote ${trials.length} trials to ${values.out}\n`);
}

main();
