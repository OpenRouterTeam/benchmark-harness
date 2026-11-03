The crash-looping pod is `ledger-worker-zx4mrndj8b-gdg8j` in the `payments` namespace (CrashLoopBackOff, 14 restarts).

It exits with code 1 right after start-up because it cannot log in to its database:

    ERROR db: connect ledger-db.payments.svc:5432: pq: password authentication failed for user "ledger"
    FATAL could not initialise ledger store, exiting (exit code 1)

The ledger database credentials need fixing (the `ledger` user's password was rejected).
