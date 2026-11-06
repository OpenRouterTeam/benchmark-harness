import { z } from "../../internal/zod";
import manifestJson from "./harness-versions.json" with { type: "json" };
import type { OriAgent } from "./schema";

const VERSION_PLACEHOLDER = "{version}";

export const AGENT_PACKAGE_PATTERN = /^[A-Za-z0-9@/:._^=+-]+$/;

export const AGENT_VERSION_PATTERN = /^[A-Za-z0-9][A-Za-z0-9.+-]*$/;

const HarnessVersionSchema = z.object({
  package: z
    .string()
    .refine(
      (value) =>
        value.includes(VERSION_PLACEHOLDER) &&
        AGENT_PACKAGE_PATTERN.test(value.replaceAll(VERSION_PLACEHOLDER, "0")),
      `package must be a valid agent package containing ${VERSION_PLACEHOLDER}`
    ),
  version: z.string().regex(AGENT_VERSION_PATTERN),
});

export type HarnessVersion = z.infer<typeof HarnessVersionSchema>;

const HarnessVersionsManifestSchema = z.object({
  node: z.string().regex(/^\d+\.\d+\.\d+$/),
  harnesses: z.object({
    claude: HarnessVersionSchema,
    pi: HarnessVersionSchema,
    "prime-agent": HarnessVersionSchema,
    omp: HarnessVersionSchema,
  } satisfies Record<OriAgent, typeof HarnessVersionSchema>),
});

export const HARNESS_VERSIONS =
  HarnessVersionsManifestSchema.parse(manifestJson);

export const HARNESS_NODE_VERSION = HARNESS_VERSIONS.node;

export function isValidAgentVersion(value: string): boolean {
  return AGENT_VERSION_PATTERN.test(value);
}

export function harnessPackage(agent: OriAgent, version?: string): string {
  const entry = HARNESS_VERSIONS.harnesses[agent];
  const resolved = version ?? entry.version;
  if (!isValidAgentVersion(resolved)) {
    throw new Error(
      `invalid agentVersion ${JSON.stringify(resolved)}: must match ${AGENT_VERSION_PATTERN.source}`
    );
  }
  return entry.package.replaceAll(VERSION_PLACEHOLDER, resolved);
}

export function defaultHarnessPackage(agent: OriAgent): string {
  return harnessPackage(agent);
}

export function resolveAgentPackage(
  agent: OriAgent,
  opts: { readonly agentPackage?: string; readonly agentVersion?: string }
): string {
  if (opts.agentPackage !== undefined && opts.agentVersion !== undefined) {
    throw new Error("set agentPackage or agentVersion, not both");
  }
  return opts.agentPackage ?? harnessPackage(agent, opts.agentVersion);
}
