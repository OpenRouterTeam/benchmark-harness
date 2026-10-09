import { writeFile } from "node:fs/promises";
import { parseArgs } from "node:util";

import { Either } from "../../../internal/either";
import { parseSchema } from "../../../internal/zod";
import { gridShapeDraws } from "../grid";
import { openRouterChat, reword } from "../realistic/rewriter";
import type { WordingEntry, WordingFile } from "../realistic/wordings";
import {
  TOOLCALL_SCHEMA_FUZZ_WORDINGS,
  unwordedShapes,
  WordingFileSchema,
} from "../realistic/wordings";

const WORDINGS_PATH = new URL("../realistic/wordings.json", import.meta.url);

const { values } = parseArgs({
  options: {
    writer: { type: "string", default: "anthropic/claude-sonnet-5.5" },
    checker: { type: "string", default: "openai/gpt-6.1-sol" },
    limit: { type: "string" },
    concurrency: { type: "string", default: "8" },
    attempts: { type: "string", default: "3" },
  },
});

const apiKey = process.env["OPENROUTER_API_KEY"];
if (apiKey === undefined) {
  process.stderr.write("OPENROUTER_API_KEY is required\n");
  process.exit(1);
}

const rewriter = {
  complete: openRouterChat({
    apiKey,
    baseUrl:
      process.env["OPENROUTER_BASE_URL"] ?? "https://openrouter.ai/api/v1",
  }),
  writer: values.writer,
  checker: values.checker,
  attempts: Number(values.attempts),
  log: (line: string): void => {
    process.stderr.write(`${line}\n`);
  },
};

const pending = unwordedShapes(TOOLCALL_SCHEMA_FUZZ_WORDINGS).slice(
  0,
  values.limit === undefined ? undefined : Number(values.limit)
);
const accepted: WordingEntry[] = [...TOOLCALL_SCHEMA_FUZZ_WORDINGS.entries];
let rejected = 0;
let writeChain = Promise.resolve();

function save(): Promise<void> {
  const order = new Map(
    gridShapeDraws().map(({ shape }, index) => [shape.key, index])
  );
  const file: WordingFile = {
    version: TOOLCALL_SCHEMA_FUZZ_WORDINGS.version + 1,
    writer: values.writer,
    checker: values.checker,
    entries: accepted.toSorted(
      (a, b) => (order.get(a.key) ?? 0) - (order.get(b.key) ?? 0)
    ),
  };
  const parsed = parseSchema(WordingFileSchema, file);
  writeChain = writeChain.then(async () =>
    Either.isLeft(parsed)
      ? undefined
      : writeFile(WORDINGS_PATH, `${JSON.stringify(parsed.right, null, 2)}\n`)
  );
  return writeChain;
}

const queue = [...pending];
await Promise.all(
  Array.from({ length: Number(values.concurrency) }, async () => {
    for (let next = queue.shift(); next !== undefined; next = queue.shift()) {
      const result = await reword(rewriter, {
        key: next.shape.key,
        drawn: next.drawn,
      });
      if (typeof result === "string") {
        rejected += 1;
        process.stderr.write(`rejected ${next.shape.key}: ${result}\n`);
        continue;
      }
      accepted.push(result);
      process.stderr.write(`accepted ${next.shape.key} (${accepted.length})\n`);
      await save();
    }
  })
);
process.stdout.write(
  `${JSON.stringify({ accepted: accepted.length, rejected, pending: pending.length })}\n`
);
