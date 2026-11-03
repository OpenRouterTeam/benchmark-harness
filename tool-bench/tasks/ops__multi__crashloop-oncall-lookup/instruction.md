PagerDuty just woke me up about ledger-worker restarting in production. Before I hand this off, I need two things:

1. Which ledger-worker pod in the production `payments` namespace is crash-looping, and the exact error it dies with (from its logs).
2. Who is on call for the ledger-worker service in PagerDuty right now: the first responder and who it escalates to next.

Write both to `/app/answer.md`. Don't restart, delete or acknowledge anything; this is just for the handoff.
