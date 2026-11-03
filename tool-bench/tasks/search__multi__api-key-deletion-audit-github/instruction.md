Our CI deploys started failing because the API key named `ci-deploy-bot` was deleted yesterday. Check the security audit logs in Elasticsearch to find who deleted it and when (UTC). Then find that person on our GitHub Enterprise: I need their GitHub username, and the number of the pull request they opened to switch our deploy pipeline over to the replacement key, plus whether it has been merged yet.

Write the user, the deletion time, their GitHub username and the pull request number and status to `/app/answer.md`. Don't change anything.
