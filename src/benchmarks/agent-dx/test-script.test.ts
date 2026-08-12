import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { agentDxTasksDir, loadTask } from "./dataset";
import { renderAgentDxTestScript } from "./test-script";
import { AGENT_DX_TEST_SCRIPTS } from "./test-script-manifest";

describe("agent-dx test scripts", () => {
  test("manifest covers exactly the task directories", () => {
    const taskDirs = readdirSync(agentDxTasksDir()).sort();
    expect(Object.keys(AGENT_DX_TEST_SCRIPTS).sort()).toEqual(taskDirs);
  });

  test("every committed test.sh matches its render (run generate-test-scripts.ts after editing)", () => {
    for (const [taskId, input] of Object.entries(AGENT_DX_TEST_SCRIPTS)) {
      const committed = readFileSync(
        join(agentDxTasksDir(), taskId, "tests", "test.sh"),
        "utf8"
      );
      expect(committed, taskId).toBe(renderAgentDxTestScript(input));
    }
  });

  test("every task verifier budget covers the app runs plus verify headroom", () => {
    const VERIFY_HEADROOM_SEC = 180;
    const appTasks = Object.entries(AGENT_DX_TEST_SCRIPTS).flatMap(
      ([taskId, input]) =>
        input.kind === "app" ? [[taskId, input] as const] : []
    );
    for (const [taskId, input] of appTasks) {
      const budget = loadTask(taskId, agentDxTasksDir()).taskToml.verifier
        .timeout_sec;
      expect(budget, taskId).toBeGreaterThanOrEqual(
        2 * input.timeoutSec + VERIFY_HEADROOM_SEC
      );
    }
  });

  test("app-style render carries the verifier protocol frame", () => {
    const script = renderAgentDxTestScript({
      kind: "app",
      description: ["desc line"],
      timeoutSec: 180,
    });
    expect(script).toStartWith(
      "#!/usr/bin/env bash\n# desc line\nset -uo pipefail\n"
    );
    expect(script).toContain("echo 0 > /logs/verifier/reward.txt");
    expect(script).toContain(
      // eslint-disable-next-line no-template-curly-in-string -- asserting shell param expansion in generated script
      'timeout "${ADX_EVAL_TIMEOUT_SEC:-180}" npm start > /logs/verifier/run.log 2>&1'
    );
    expect(script).toContain("if ! run_app && ! run_app; then");
    expect(script).toContain("sed 's/^/[app] /' /logs/verifier/run.log");
    expect(script).toContain(
      'echo "SUBCHECK verified=pass"\n  echo 1 > /logs/verifier/reward.txt'
    );
  });

  test("app-style render supports run args and cleanup", () => {
    const script = renderAgentDxTestScript({
      kind: "app",
      description: ["desc"],
      timeoutSec: 240,
      runArgs: " -- /tests/fixture.png",
      cleanupLines: ["cleanup_step"],
    });
    expect(script).toContain("npm start -- /tests/fixture.png >");
    expect(script).toContain(
      "cleanup() {\n  cleanup_step\n}\ntrap cleanup EXIT\n"
    );
    const trapIndex = script.indexOf("trap cleanup EXIT");
    expect(trapIndex).toBeGreaterThan(-1);
    expect(trapIndex).toBeLessThan(script.indexOf("run_app()"));
  });

  test("answer-style render checks the answer file and skips the fresh run", () => {
    const script = renderAgentDxTestScript({
      kind: "answer",
      description: ["desc"],
      answerFile: "RECOMMENDATION.md",
    });
    expect(script).toContain("if [ -f RECOMMENDATION.md ]; then");
    expect(script).toContain(
      'echo "RECOMMENDATION.md not found in /app"\n  exit 0'
    );
    expect(script).not.toContain("run_app");
    expect(script).toContain("echo 0 > /logs/verifier/reward.txt");
    expect(script).toContain(
      'echo "SUBCHECK verified=pass"\n  echo 1 > /logs/verifier/reward.txt'
    );
  });
});
