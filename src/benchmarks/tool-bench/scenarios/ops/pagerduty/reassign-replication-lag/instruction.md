I just got paged for replication lag on orders-db (replica-2). This one is a database problem, not a platform one: please reassign that incident in PagerDuty to whoever is the primary on-call for Database Reliability right now, and add this note to it: "Handing off to DBRE: replica-2 disk is at 100%, needs a volume resize."

Leave my other incidents alone, and don't acknowledge or resolve anything.
