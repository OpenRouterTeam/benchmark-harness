Model IDs are hardcoded across a dozen of our services, and every model swap is a multi-repo change. We want the model choice centralized on OpenRouter so services just reference it. Build a minimal TypeScript Node project in the current directory that:

1. Creates (or updates) an OpenRouter preset with the slug from the ADX_PRESET_SLUG environment variable, configuring a current, valid chat model of your choice and a short system prompt.
2. Runs one chat completion through that preset (referencing the preset, not the underlying model id directly) asking the model to reply with exactly the word `ready`.

The API key is in the OPENROUTER_API_KEY environment variable. `npm start` must print `PRESET_MODEL <model id configured in the preset>`, the raw preset object returned by the presets API as JSON, and the raw chat completion response as JSON.
