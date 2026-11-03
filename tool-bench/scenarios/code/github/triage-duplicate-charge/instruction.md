Support just escalated a bug on our GitHub Enterprise: customers are being charged twice when a card authorisation is retried. There should already be an open issue for it in one of our payments repositories (ignore anything that's already closed or that's about something else).

Please triage it: add the `sev-1` label without removing the labels it already has, and assign it to the engineer who most recently changed the charge retry logic (`internal/charge/retry.go` in `octo-org/payments-api`). Don't create a new issue.
