import { readFileSync, writeFileSync } from "node:fs";

const RUN_LOG = "/logs/verifier/run.log";

const PASSAGE =
  "The coral reef restoration project off the coast of Belize planted over sixty thousand nursery-grown staghorn fragments this year, and early surveys show survival rates above eighty percent, which the marine biologists attribute to cooler-than-average water temperatures and a new outplanting technique.";

const TOPIC_MARKERS = [/reef/i, /coral/i, /staghorn/i, /belize/i] as const;

function fail(
  message: string,
  kind: "agent" | "platform" | "fixture" = "agent"
): never {
  writeFileSync(
    "/logs/verifier/verdict.json",
    JSON.stringify({ kind, detail: message })
  );
  console.error(`VERIFY FAIL: ${message}`);
  process.exit(1);
}

function main(): void {
  const output = readFileSync(RUN_LOG, "utf8");

  const modelLine = output.match(/^MODEL\s+(\S+)/m);
  if (modelLine?.[1] === undefined) {
    fail("output has no MODEL <id> line");
  }

  const summaryLine = output.match(/^SUMMARY\s+(.+)$/m);
  const summary = summaryLine?.[1]?.trim();
  if (summary === undefined || summary === "") {
    fail("output has no SUMMARY <one sentence> line");
  }
  if (summary.length >= PASSAGE.length) {
    fail("summary is not shorter than the passage");
  }
  if (summary.split(/\s+/).length < 5) {
    fail("summary is too short to be a real sentence");
  }
  if (PASSAGE.toLowerCase().includes(summary.toLowerCase())) {
    fail("summary is a verbatim excerpt of the passage, not a model summary");
  }
  if (!TOPIC_MARKERS.some((marker) => marker.test(summary))) {
    fail("summary does not mention the passage topic");
  }

  console.log(`MODEL ${modelLine[1]}`);
  console.log("VERIFY PASS");
}

main();
