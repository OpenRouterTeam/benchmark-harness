import { Either } from "../../internal/either";
import { firstZodIssueMessage, parseSchema, z } from "../../internal/zod";
import imageIdsJson from "./image-ids.json";
import { TERMINAL_BENCH_4_SOURCE_COMMIT } from "./tasks-source";

const ModalImageIdSchema = z.string().regex(/^im-[A-Za-z0-9]+$/);

const TaskImagesSchema = z.object({
  agent: ModalImageIdSchema,
  verifier: ModalImageIdSchema,
});

export type TerminalBench4TaskImages = z.infer<typeof TaskImagesSchema>;

const ImageIdsSchema = z.object({
  sourceCommit: z.string().regex(/^[0-9a-f]{40}$/),
  images: z.record(z.string().min(1), TaskImagesSchema),
});

export interface TerminalBench4ImageMap {
  readonly sourceCommit: string;
  readonly images: ReadonlyMap<string, TerminalBench4TaskImages>;
}

export function buildImageMap(
  raw: unknown,
  expectedCommit: string = TERMINAL_BENCH_4_SOURCE_COMMIT
): TerminalBench4ImageMap {
  const parsed = parseSchema(ImageIdsSchema, raw);
  if (Either.isLeft(parsed)) {
    throw new TypeError(
      `terminal-bench-4 image-ids.json is invalid: ${firstZodIssueMessage(parsed.left)}`
    );
  }
  const { sourceCommit, images } = parsed.right;
  if (sourceCommit !== expectedCommit) {
    throw new TypeError(
      `terminal-bench-4 image-ids.json was built from ${sourceCommit} but tasks are pinned to ${expectedCommit}; rerun scripts/build-terminal-bench-4-images.py`
    );
  }
  return { sourceCommit, images: new Map(Object.entries(images)) };
}

export function taskImages(
  map: TerminalBench4ImageMap,
  taskId: string
): TerminalBench4TaskImages | undefined {
  return map.images.get(taskId);
}

export const TERMINAL_BENCH_4_IMAGES: TerminalBench4ImageMap =
  buildImageMap(imageIdsJson);
