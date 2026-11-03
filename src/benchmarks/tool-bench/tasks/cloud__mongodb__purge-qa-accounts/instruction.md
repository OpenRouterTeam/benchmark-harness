Our QA team accidentally ran their signup tests against production, so the `customers` collection in the production `crm` database (MongoDB) now has test accounts in it. Delete every customer whose email address is on the `@qa.octoshop.test` domain.

Keep everyone else — including real customers or staff whose address just happens to contain "qa" — and leave the `crm_staging` database alone.
