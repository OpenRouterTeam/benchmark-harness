import { readFileSync, writeFileSync } from "node:fs";

const ANSWER_PATH = "/app/ANSWER.md";

interface TopicCheck {
  readonly topic: string;
  readonly pattern: RegExp;
}

const REQUIRED_TOPICS: readonly TopicCheck[] = [
  {
    topic: "BYOK mechanism named",
    pattern: /byok|bring your own key|provider key|integration/i,
  },
  {
    topic: "where to configure",
    pattern: /settings|dashboard|integrations|openrouter\.ai\/settings/i,
  },
  {
    topic: "how usage is visible",
    pattern: /generation|response|record|activity|header/i,
  },
  {
    topic: "failure/rate-limit behavior",
    pattern: /rate.?limit|fail|fallback|error/i,
  },
  { topic: "BYOK fee", pattern: /fee|5\s?%|percent|charge|surcharge/i },
];

function fail(
  message: string,
  kind: "agent" | "platform" | "fixture" = "agent"
): never {
  const singleLine = message.replaceAll(/\r?\n/g, "\\n");
  writeFileSync(
    "/logs/verifier/verdict.json",
    JSON.stringify({ kind, detail: singleLine })
  );
  console.error(`VERIFY FAIL: ${singleLine}`);
  process.exit(1);
}

function main(): void {
  const answer = readFileSync(ANSWER_PATH, "utf8");
  if (answer.trim().length < 300) {
    fail("ANSWER.md is too short to be a usable runbook");
  }

  const missing = REQUIRED_TOPICS.filter(
    (check) => !check.pattern.test(answer)
  );
  if (missing.length > 0) {
    fail(
      `runbook does not cover: ${missing.map((check) => check.topic).join("; ")}`
    );
  }

  console.log(
    "runbook covers configuration, visibility, failure behavior, and fees"
  );
  console.log("VERIFY PASS");
}

main();
