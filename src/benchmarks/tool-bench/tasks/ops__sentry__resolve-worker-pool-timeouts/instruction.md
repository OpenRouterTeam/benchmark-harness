Dmitri's fix for the payments worker's database connection-pool timeouts ("could not obtain connection from pool") shipped in `payments-worker@1.15.0`, and it's live in production now. Please mark that issue as resolved in Sentry (org `lumen-labs`).

Don't change anything else. The checkout API has a similar-looking error that's still being investigated.
