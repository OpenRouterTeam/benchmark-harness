PagerDuty paged us about the checkout error rate in production. Please find the error behind it in Sentry (org `lumen-labs`) and open a GitHub issue in `lumen-labs/checkout` so the payments team can fix it:

- the title should start with the Sentry short issue ID (e.g. `PROJECT-123: ...`)
- the description must include the PagerDuty incident number, the Sentry culprit and the error message

Then add a note to the PagerDuty incident with the number of the GitHub issue you opened.

Staging has a similar alert from a load test; ignore it. Don't change the status of any incident or Sentry issue.
