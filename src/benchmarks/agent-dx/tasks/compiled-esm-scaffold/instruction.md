Build a minimal TypeScript Node project in the current directory that calls the OpenRouter chat completions API.

Requirements:

- The TypeScript sources must be compiled with `tsc` as part of `npm start` (a `prestart` or chained build step is fine). Do not run the TypeScript directly with a loader like `tsx` or `ts-node`.
- The entry module must use top-level `await` for the API call (no `main().then(...)` wrapper).

The API key is in the OPENROUTER_API_KEY environment variable. `npm start` should make one chat completion request and print the raw API response as JSON.
