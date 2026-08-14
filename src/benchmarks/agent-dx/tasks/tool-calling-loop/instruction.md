Build a minimal TypeScript Node project in the current directory that uses tool calling through the OpenRouter chat completions API.

The API key is in the OPENROUTER_API_KEY environment variable. Give the model a `lookup_order` tool that returns the shipping status for an order id (every order is shipped and arrives in 3 days), ask it "What is the status of order #4242 and when will it arrive?", and run the tool-calling loop until the model gives a final answer. `npm start` should run this end to end, printing each raw API response as JSON and then the final answer.
