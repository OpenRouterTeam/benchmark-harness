The production orders API workspace in our Commerce project (HCP Terraform org `acme-platform`) has been stuck since last night: the run's worker crashed mid-apply and the workspace is still locked, so nobody can queue anything.

Please clear that lock and then queue a plan-only run on it so we can check whether anything drifted.

Heads up: there's also an old legacy orders prod workspace in the Legacy project that Dmitri locked on purpose for a data migration. Do not touch that one, and don't apply or delete anything.
