Billing API latency in production has been bad for a good part of today and the p95 alert in Datadog is firing. Use the Datadog latency data to work out when the regression actually started, then find the billing-api release that went out in Argo CD right before it. Roll the production billing-api app back to the release that was running before that one, by pinning it to that release's git tag instead of `main`, and make sure it actually gets deployed. Keep automated sync on.

Don't touch the staging billing-api app or any other application.
