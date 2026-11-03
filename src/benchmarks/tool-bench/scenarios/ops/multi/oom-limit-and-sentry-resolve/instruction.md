The quarterly reports didn't go out again: something in the `analytics` namespace of our production cluster keeps getting OOM-killed overnight. Find the workload that is being killed and raise its container memory limit to 1Gi.

Each kill also shows up in Sentry (org `lumen-labs`) as a worker-lost error for that service. Once the limit is raised, mark that Sentry issue as resolved.

Only change the workload that is actually being killed, leave staging alone, and don't touch Sentry issues that belong to other services or other errors.
