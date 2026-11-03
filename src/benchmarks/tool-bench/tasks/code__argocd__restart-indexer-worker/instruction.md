We rotated the Elasticsearch password for the search indexer this morning, and the production indexer worker pods are still holding the old credentials, so every bulk write is failing with 401s. Please restart the production search-indexer worker Deployment through Argo CD so the pods pick up the new secret.

Only the worker needs a restart; leave the indexer API alone, and don't touch staging. The production app is showing OutOfSync because of a config change that hasn't been reviewed yet, so do not sync or edit the application.
