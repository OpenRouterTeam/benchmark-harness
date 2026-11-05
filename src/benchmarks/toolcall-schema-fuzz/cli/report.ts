import { readFile } from "node:fs/promises";

import * as Result from "effect/Result";

import {
  asyncBufferFromBytes,
  parquetErrorMessage,
  readResultRows,
} from "../../../results/parquet";
import { stabilityReport } from "../stability";

const paths = process.argv.slice(2);
if (paths.length === 0) {
  process.stderr.write("usage: report.ts <results.parquet>...\n");
  process.exit(1);
}

const rowLists = Result.all(
  await Promise.all(
    paths.map(async (path) =>
      readResultRows(asyncBufferFromBytes(await readFile(path)))
    )
  )
);
if (Result.isFailure(rowLists)) {
  process.stderr.write(`${parquetErrorMessage(rowLists.failure)}\n`);
  process.exit(1);
}
process.stdout.write(
  `${JSON.stringify(stabilityReport(rowLists.success.flat()), null, 2)}\n`
);
