The v1.43.0 release of billing-worker is crash-looping in production (it expects a database migration that hasn't run yet). Please roll the production billing-worker app in Argo CD back to the v1.42.0 release by pinning it to the `v1.42.0` git tag instead of `main`, and make sure it actually gets deployed. Keep automated sync turned on for it.

Leave billing-api and the staging billing-worker alone; they're fine on main.
