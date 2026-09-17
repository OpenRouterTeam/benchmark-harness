import { z } from "../../internal/zod";

export const DEFAULT_MODAL_ENV = "main" as const;

/** Broad US placement (1.15x compute) so provider egress originates in the US. */
export const DEFAULT_MODAL_REGIONS = ["us"] as const;

/** Shared Modal sandbox placement options for Modal-backed benchmarks. */
export const ModalSandboxOptionsSchema = z.object({
  modalEnv: z.string().default(DEFAULT_MODAL_ENV),
  modalRegions: z
    .array(z.string().min(1))
    .default(() => [...DEFAULT_MODAL_REGIONS]),
});
