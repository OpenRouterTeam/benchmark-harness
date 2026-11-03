PR #881 for payments-api (the 3.18.0 bump that also drops the old legacy-webhooks config) was merged to main about an hour ago, and Argo CD now shows payments-api as OutOfSync. Please sync the production payments-api so it's running what's on main, and make sure the `payments-api-legacy-webhooks` ConfigMap that the PR removed actually gets deleted from the cluster.

Staging payments-api is frozen for a QA run today, so don't sync or change it.
