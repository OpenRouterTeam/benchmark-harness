Tonight we're doing storage maintenance on the Postgres replica pg-replica-2 (change CHG-7781), from 02:00 to 04:00 UTC on 13 Nov 2026. Please add a Grafana alerting silence so alerts with alertname `PostgresReplicationLag` and instance `pg-replica-2` stay quiet for exactly that window, with the change ticket in the comment.

Only that replica: pg-replica-1 must keep alerting. Don't touch any existing silences.
