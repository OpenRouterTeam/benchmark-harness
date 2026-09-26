import { z } from "../internal/zod";

export const DEFAULT_MODAL_ENV = "main" as const;

const MODAL_REGION_PATTERN = /^\S+$/;

export const ModalSandboxOptionsSchema = z.object({
  modalEnv: z.string().default(DEFAULT_MODAL_ENV),
  modalRegions: z
    .array(
      z
        .string()
        .regex(
          MODAL_REGION_PATTERN,
          "region must not be blank or contain whitespace"
        )
    )
    .optional(),
});
