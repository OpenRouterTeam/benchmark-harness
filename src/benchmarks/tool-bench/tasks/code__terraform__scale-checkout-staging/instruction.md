We're load testing checkout in staging this week. In our HCP Terraform org `acme-platform`, please bump the checkout service's desired task count in the staging workspace from 2 to 4, then kick off a normal plan-and-apply run with the message "Scale checkout staging to 4 tasks for load test".

I want to review the plan myself before anything is applied, so don't approve it. Production must not be touched.
