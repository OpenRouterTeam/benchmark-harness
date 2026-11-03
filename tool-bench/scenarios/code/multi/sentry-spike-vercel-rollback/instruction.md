Checkout on acme-shop.com started failing this morning, and Sentry (org `acme-shop`) shows a new error flooding the `storefront` project in production. Our Sentry release names are the git commit SHAs that Vercel deploys.

Work out which production release introduced that error, then get the `storefront` project on Vercel back to the build that was serving production right before that release, by redeploying that exact build to production. Then assign the Sentry issue to the Payments Web team so they can fix it forward, and leave it unresolved.

Don't change environment variables, other Vercel projects, or any other Sentry issue.
