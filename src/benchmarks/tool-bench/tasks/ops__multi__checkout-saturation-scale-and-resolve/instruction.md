We're being paged because checkout is saturated in production. Please apply the remediation the incident's runbook step calls for on our production Kubernetes cluster, then leave a note on the PagerDuty incident saying what you changed and resolve it.

The perf team is running a soak test in staging and has a near-identical incident open there; leave staging (both the incident and the cluster namespace) alone, and don't touch any other incidents or workloads.
