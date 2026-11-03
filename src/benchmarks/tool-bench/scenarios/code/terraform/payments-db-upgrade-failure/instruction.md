The most recent Terraform run on our `payments-api-prod` workspace (HCP Terraform org `acme-platform`) failed this afternoon and the payments team is asking what happened. Please don't change anything — just investigate.

Write a short summary to `/app/answer.md` that includes:

- the ID of the failed run,
- the resource address that failed and the AWS error code from the failure,
- what that run's plan said it would do, written exactly in Terraform's format, e.g. `Plan: 2 to add, 0 to change, 1 to destroy`.
