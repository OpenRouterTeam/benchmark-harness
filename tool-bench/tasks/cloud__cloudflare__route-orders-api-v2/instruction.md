The new `orders-api-v2` Worker is deployed in our Cloudflare account and has passed staging.
Please start sending production v2 traffic to it: requests matching
`api.octo-corp.com/v2/*` should be handled by `orders-api-v2`.

Everything else on api.octo-corp.com (including /v1) must keep going to the current
`orders-api` Worker, and don't touch the staging setup on octo-corp.dev.
