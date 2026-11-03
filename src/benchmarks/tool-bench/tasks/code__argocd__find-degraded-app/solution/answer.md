Degraded production app: checkout-web (Argo CD project storefront, cluster prod-us-east-1, namespace storefront).

Failing workload: Deployment checkout-web. Its new ReplicaSet checkout-web-7f8b9c6d5 (the 2.14.0-rc3 rollout) has pod checkout-web-7f8b9c6d5-q2x7m stuck in ImagePullBackOff, so the rollout exceeded its progress deadline. The old 2.13.4 pods are still serving.

Root cause: the image ghcr.io/lumen-retail/checkout-web:2.14.0-rc3 cannot be pulled; the registry answers "not found" for that tag. The commit "checkout-web(prod): promote 2.14.0-rc3" points production at an image tag that was never pushed.
