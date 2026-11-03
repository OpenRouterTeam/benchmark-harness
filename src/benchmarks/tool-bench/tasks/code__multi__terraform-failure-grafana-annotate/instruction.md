Our Postgres 16 upgrade of the production payments database failed in HCP Terraform this afternoon (org `acme-platform`). I want the DB graphs in Grafana to show exactly when it happened so we can line it up with the connection errors.

Find the failed run on the production payments workspace and add one annotation to the production "Payments DB" dashboard in Grafana, at the exact time (to the second) that run errored. The annotation text must include the run ID and the AWS error code from the failure, and tag it `terraform` and `payments`.

Don't change anything in Terraform, don't edit any dashboards, and leave the staging and legacy dashboards alone.
