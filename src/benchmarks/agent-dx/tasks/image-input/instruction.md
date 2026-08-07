Build a minimal TypeScript Node project in the current directory that describes a local image file using a vision-capable model through the OpenRouter chat completions API.

The API key is in the OPENROUTER_API_KEY environment variable. `npm start -- <path-to-image>` should send that image to the model, print the raw API response as JSON, and then print the model's description of the image.
