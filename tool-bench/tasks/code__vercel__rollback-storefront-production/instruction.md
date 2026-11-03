Checkout on acme-shop.com has been throwing errors since this morning's production deploy of "Switch to payments client v3 (#491)" on our `storefront` Vercel project. We'll fix it forward later. For now, get production back to the build that was serving traffic before that deploy, by redeploying that exact build to production.

Don't change environment variables or any other project.
