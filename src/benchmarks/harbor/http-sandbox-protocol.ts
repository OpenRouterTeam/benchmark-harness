import { z } from "../../internal/zod";

export const CreateSandboxRequestSchema = z.object({
  imageTag: z.string().min(1),
  imageBuildSteps: z.array(z.string()).optional(),
  timeoutSec: z.number().int().positive(),
  cpus: z.number().positive(),
  memoryMb: z.number().int().positive(),
  allowInternet: z.boolean(),
  workdir: z.string().min(1),
  command: z.array(z.string()).min(1),
});

export type CreateSandboxRequest = z.infer<typeof CreateSandboxRequestSchema>;

export const CreateSandboxResponseSchema = z.object({
  sandboxId: z.string().min(1),
  hostUrl: z.string().min(1),
});

export type CreateSandboxResponse = z.infer<typeof CreateSandboxResponseSchema>;

export const SANDBOX_STATUSES = [
  "creating",
  "running",
  "stopped",
  "failed",
] as const;

export type SandboxStatus = (typeof SANDBOX_STATUSES)[number];

export const SandboxStatusResponseSchema = z.object({
  sandboxId: z.string().min(1),
  status: z.enum(SANDBOX_STATUSES),
  error: z.string().optional(),
});

export type SandboxStatusResponse = z.infer<typeof SandboxStatusResponseSchema>;

export const StartExecRequestSchema = z.object({
  argv: z.array(z.string()).min(1),
  env: z.record(z.string(), z.string()),
  timeoutMs: z.number().int().positive(),
});

export type StartExecRequest = z.infer<typeof StartExecRequestSchema>;

export const StartExecResponseSchema = z.object({
  execId: z.string().min(1),
});

export type StartExecResponse = z.infer<typeof StartExecResponseSchema>;

export const ExecStatusResponseSchema = z.discriminatedUnion("status", [
  z.object({ status: z.literal("running") }),
  z.object({
    status: z.literal("done"),
    stdout: z.string(),
    stderr: z.string(),
    exitCode: z.number().int(),
  }),
]);

export type ExecStatusResponse = z.infer<typeof ExecStatusResponseSchema>;

export const ErrorResponseSchema = z.object({
  error: z.string(),
});

const SANDBOX_ID_SEPARATOR = "#";

export function encodeHttpSandboxId(hostUrl: string, localId: string): string {
  return `${hostUrl}${SANDBOX_ID_SEPARATOR}${localId}`;
}

export interface DecodedHttpSandboxId {
  readonly hostUrl: string;
  readonly localId: string;
}

export function decodeHttpSandboxId(
  sandboxId: string
): DecodedHttpSandboxId | undefined {
  const separatorIndex = sandboxId.lastIndexOf(SANDBOX_ID_SEPARATOR);
  if (separatorIndex <= 0 || separatorIndex === sandboxId.length - 1) {
    return undefined;
  }
  const hostUrl = sandboxId.slice(0, separatorIndex);
  const localId = sandboxId.slice(separatorIndex + 1);
  if (!hostUrl.startsWith("http://") && !hostUrl.startsWith("https://")) {
    return undefined;
  }
  return { hostUrl, localId };
}
