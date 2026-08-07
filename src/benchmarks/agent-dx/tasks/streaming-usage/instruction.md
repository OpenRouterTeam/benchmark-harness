Build a minimal TypeScript Node project in the current directory that streams a chat completion from the OpenRouter API.

The API key is in the OPENROUTER_API_KEY environment variable. `npm start` should make one streaming chat completion request, print the response text to stdout as it streams in, and when the stream ends print the response id plus the total tokens used and the cost of the request.
