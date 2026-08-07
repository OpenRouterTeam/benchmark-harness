import { writeFileSync } from "node:fs";
import { join } from "node:path";

import { iLog } from "../../internal/log";
import { agentDxTasksDir } from "./dataset";
import { renderAgentDxTestScript } from "./test-script";
import { AGENT_DX_TEST_SCRIPTS } from "./test-script-manifest";

for (const [taskId, input] of Object.entries(AGENT_DX_TEST_SCRIPTS)) {
  writeFileSync(
    join(agentDxTasksDir(), taskId, "tests", "test.sh"),
    renderAgentDxTestScript(input)
  );
}
iLog("agent-dx: test scripts generated", {
  count: Object.keys(AGENT_DX_TEST_SCRIPTS).length,
});
