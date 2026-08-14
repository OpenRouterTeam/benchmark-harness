Build a research agent as a minimal TypeScript Node project in the current directory, using OpenRouter and whatever OpenRouter tooling fits the job best.

The agent must run a multi-step workflow: the model plans, calls local tools, receives their results, and keeps going until it can produce a final report. Do not compute the answer yourself in code — the model must drive the workflow through tool calls.

Requirements:

- A tool registry exposing exactly two tools to the model:
  - `lookup_datacenter(site_id)` — returns the record from this table:
    - `DC-A`: region eu-west, servers 1200, avg_watts_per_server 350
    - `DC-B`: region us-east, servers 3400, avg_watts_per_server 290
    - `DC-C`: region ap-south, servers 800, avg_watts_per_server 410
  - `calculate(expression)` — evaluates a basic arithmetic expression and returns the result.
- Stream the model's output (print `STREAM_CHUNKS <count>` with the number of streamed chunks received across the run).
- A cost stop condition: the run must abort if total spend exceeds $0.50 (print `BUDGET_USD 0.50` at start).
- Per-step usage accounting: after every model turn print `STEP_USAGE <step> <prompt_tokens> <completion_tokens> <cost_usd>` using the provider's reported usage, not estimates.
- The final report must be produced as structured output conforming to this JSON schema: an object with `highest_power_site` (string), `total_kilowatts` (number), and `steps_taken` (integer).

The research question: "Which datacenter draws the most total power, and what is the combined power draw of all three sites in kilowatts?"

The API key is in the OPENROUTER_API_KEY environment variable. `npm start` must print the raw API response as JSON for every model turn, a line `TOOL_CALL <tool name> <arguments json>` for each tool invocation, the usage and streaming lines above, and finally `REPORT_JSON <the report as single-line JSON>`.
