import { readFile } from "node:fs/promises";

import { asyncBufferFromBytes, readResultRows } from "../../../results/parquet";
import { stabilityReport } from "../stability";

const paths = process.argv.slice(2);
if (paths.length === 0) {
  process.stderr.write("usage: report.ts <results.parquet>...\n");
  process.exit(1);
}

const rowLists = await Promise.all(
  paths.map(async (path) =>
    readResultRows(asyncBufferFromBytes(await readFile(path)))
  )
);
process.stdout.write(
  `${JSON.stringify(stabilityReport(rowLists.flat()), null, 2)}\n`
);
