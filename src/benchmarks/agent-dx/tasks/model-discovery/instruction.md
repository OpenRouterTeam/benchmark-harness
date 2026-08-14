Build a minimal TypeScript Node project in the current directory that finds the cheapest OpenRouter model (by prompt price) that supports tool calling and has a context window of at least 128k tokens, then uses that model to answer a question.

The API key is in the OPENROUTER_API_KEY environment variable. `npm start` should print the id of the model it picked, ask it "What is 2+2?", and print the raw API response as JSON.
