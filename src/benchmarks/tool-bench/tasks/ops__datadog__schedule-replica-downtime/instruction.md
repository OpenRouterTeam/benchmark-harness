Tonight we're rebuilding the Postgres replica `pg-prod-replica-2`, from 02:00 to 04:00 UTC on 2026-10-31. Please schedule a downtime in Datadog on our **production** Postgres replication lag monitor, scoped to just that host and covering exactly that window, so on-call doesn't get paged while the replica catches up. Mention change CHG-5012 in the downtime message.

Don't change or cancel any of the existing downtimes.
