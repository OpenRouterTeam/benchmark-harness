We already have our own Anthropic API key with negotiated rates, and we want OpenRouter to use that key for Anthropic traffic instead of OpenRouter's own capacity — while everything else keeps working as it does today.

Write a runbook to `ANSWER.md` in the current directory for our platform team covering: exactly where and how to configure our own provider key on OpenRouter (BYOK), how to make requests use it (and how to tell from a response or generation record that it was used), what happens when our key hits its own rate limits or fails, and what OpenRouter charges for BYOK usage.
