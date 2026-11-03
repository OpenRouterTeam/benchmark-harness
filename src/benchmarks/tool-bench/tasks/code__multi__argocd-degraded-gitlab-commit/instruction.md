Checkout in production has been flaky since the lunchtime deploy, and Argo CD shows one of our production applications as Degraded. Find out which production app it is and what is actually wrong with it, then trace the deployed config change back through our GitLab (the deploy repo and the app's CI) to explain how a broken release got promoted.

Please don't change, sync or retry anything. Write a short summary to `/app/answer.md` with:

- the Degraded production application and the image tag it is failing on,
- the ID of the GitLab pipeline that promoted that release, and the name of the job in it that failed,
- the error that job failed with.
