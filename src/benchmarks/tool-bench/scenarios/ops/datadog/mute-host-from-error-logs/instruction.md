Since about 13:00 UTC today (2026-10-30), one of our production checkout API hosts keeps logging "connection reset by peer" errors. We're draining it and replacing it.

Use the logs in Datadog to work out which production host it is, then mute that host in Datadog until 18:00 UTC today with a message that references INC-2291. Don't mute or unmute any other hosts.
