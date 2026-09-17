import { z } from "../../internal/zod";

export const DEFAULT_MODAL_ENV = "main" as const;

export const DEFAULT_MODAL_REGIONS = ["us"] as const;

export const ModalSandboxOptionsSchema = z.object({
  modalEnv: z.string().default(DEFAULT_MODAL_ENV),
  modalRegions: z
    .array(z.string().min(1))
    .default(() => [...DEFAULT_MODAL_REGIONS]),
});
