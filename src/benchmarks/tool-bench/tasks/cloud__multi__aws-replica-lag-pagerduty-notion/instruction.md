I'm taking PagerDuty incident #4812, the replication lag page for orders-db-replica-2. Can you do the paperwork for me?

1. Find the CloudWatch alarm that fired for that replica and add a note to the PagerDuty incident with the alarm's name and when it went into ALARM.
2. Start a postmortem entry for it in the Postmortems database in Notion, like last week's one for replica-1: title "orders-db-replica-2 replication lag", Incident "PagerDuty #4812", Severity SEV2, Status Draft, Started set to the day the alarm fired, and the alarm name in the Alarm field.

Don't acknowledge or resolve anything, I'll do that myself.
