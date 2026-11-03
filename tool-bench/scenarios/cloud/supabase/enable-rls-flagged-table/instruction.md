The Supabase security advisor is flagging a table in our production storefront project. Please fix that finding by turning on row level security for the table. Only our backend reads it, using the service role, so don't add any policies.

Make the change as a tracked database migration so it shows up in our migration history, and don't touch the staging project.
