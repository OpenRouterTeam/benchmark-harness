Crash-looping pod: `ledger-worker-zx4mrndj8b-gdg8j` in the production `payments` namespace (CrashLoopBackOff).
It exits right after start-up because the ledger database rejects its credentials:

    ERROR db: connect ledger-db.payments.svc:5432: pq: password authentication failed for user "ledger"
    FATAL could not initialise ledger store, exiting (exit code 1)

On call for the ledger-worker service (escalation policy "Payments") right now:
- Level 1: Priya Raman (override covering Riya Patel until 2026-10-31 06:00 UTC)
- Level 2: Marcus Chen
