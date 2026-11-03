Payments signed off on the new checkout. Please start rolling it out to a quarter of our
production shoppers.

The storefront reads its feature flags from the `storefront-flags` KV store in our
production Cloudflare account ("Octo Corp"). The `checkout_v2` flag there is a small JSON
object: set `enabled` to true and `rollout` to 25, and keep its other fields exactly as they
are. The staging account and the preview flags must stay as they are.
