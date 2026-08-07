Build a minimal TypeScript Node project in the current directory that calls the OpenRouter chat completions API and is resilient to model failures: if the primary model errors or is unavailable, it should automatically fall back to the backup model and clearly report what happened.

The API key is in the OPENROUTER_API_KEY environment variable, and the model ids come from the PRIMARY_MODEL and FALLBACK_MODEL environment variables. `npm start` should ask "What is the capital of France?" and print the raw API response as JSON along with a note about which model actually answered and why.
