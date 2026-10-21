import { createHash } from "node:crypto";

import type { HttpClient } from "@effect/platform";
import type { Effect } from "effect/Effect";
import { fail, gen, mapError } from "effect/Effect";

import { fetchCachedTextFile } from "../../datasets/cached-file";
import { SolverError } from "../../harness/core";
import type { RetryConfig } from "../../runtime/retry";
import type { RecoveryBenchSampleMeta } from "./dataset";

export function sha256Hex(bytes: Uint8Array | string): string {
  return createHash("sha256").update(bytes).digest("hex");
}

export function verifyTrajectoryBytes(
  meta: Pick<
    RecoveryBenchSampleMeta,
    "trialDir" | "trajectorySha256" | "trajectoryBytes"
  >,
  text: string
): string | undefined {
  const bytes = Buffer.from(text, "utf8");
  if (bytes.byteLength !== meta.trajectoryBytes) {
    return `trajectory for ${meta.trialDir} has ${bytes.byteLength} bytes, manifest expects ${meta.trajectoryBytes}`;
  }
  const digest = sha256Hex(bytes);
  if (digest !== meta.trajectorySha256) {
    return `trajectory for ${meta.trialDir} sha256 ${digest} does not match manifest ${meta.trajectorySha256}`;
  }
  return undefined;
}

export function fetchTrajectoryText(
  meta: RecoveryBenchSampleMeta,
  retry?: RetryConfig
): Effect<string, SolverError, HttpClient.HttpClient> {
  return gen(function* () {
    const text = yield* fetchCachedTextFile({
      url: meta.trajectoryUrl,
      ...(retry === undefined ? {} : { retry }),
      validate: (candidate) => verifyTrajectoryBytes(meta, candidate),
    }).pipe(
      mapError(
        (e) =>
          new SolverError({
            message: `Failed to fetch recovery-bench trajectory ${meta.trajectoryUrl}: ${e.message}`,
          })
      )
    );
    const invalid = verifyTrajectoryBytes(meta, text);
    if (invalid !== undefined) {
      return yield* fail(new SolverError({ message: invalid }));
    }
    return text;
  });
}
