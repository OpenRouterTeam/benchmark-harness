Build a minimal TypeScript Node project in the current directory that extracts structured data from free-text receipts using the OpenRouter chat completions API. It must reliably return valid JSON with the fields `merchant`, `date`, and `total` — it should never crash on malformed model output.

The API key is in the OPENROUTER_API_KEY environment variable. `npm start` should extract from this receipt, print the raw API response as JSON, and then print the extracted JSON object:

```
BLUE BOTTLE COFFEE
Date: 2026-07-12
1x Iced Latte      $6.50
1x Croissant       $4.25
Tip                $3.75
TOTAL             $14.50
```
