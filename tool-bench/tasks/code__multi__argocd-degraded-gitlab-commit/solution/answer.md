Degraded production app: checkout-web (cluster prod-us-east-1, namespace storefront). Its new ReplicaSet
cannot pull ghcr.io/lumen-retail/checkout-web:2.14.0-rc3 (ErrImagePull / ImagePullBackOff, "not found").

Argo CD is synced to storefront-deploy commit a23f2a7 "checkout-web(prod): promote 2.14.0-rc3", pushed by
deploy-bot from lumen-retail/storefront/checkout-web pipeline 7712 (tag v2.14.0-rc3).

In pipeline 7712 the image-publish job failed: `docker push` was rejected with
"denied: permission_denied: The token provided does not match expected scopes (write:packages)" because the
GHCR_TOKEN CI token expired, so the 2.14.0-rc3 image was never pushed. The promote job runs with
`when: always`, so it promoted the tag to production anyway.
