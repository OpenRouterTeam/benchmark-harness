The data engineering team has finished migrating off the old reporting worker in production (the EC2 instance named `reporting-worker-legacy`). They want it shut off now, but they need its disk kept for another month in case they have to pull files from it, so stop it rather than terminating it.

Also tag the instance with `DecommissionTicket` = `OPS-4821` so everyone knows why it's down. There's a staging copy with the same name; leave that one, and every other instance, running as they are.
