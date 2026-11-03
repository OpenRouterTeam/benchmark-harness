Priya Shah emailed our ops inbox this week: the desk lamp from one of her orders arrived broken and she wants a full refund.

Please handle it end to end:

- Find the order she means in our storefront database (the production Supabase project) and refund its payment in full in Stripe.
- Mark that order as refunded in the database.
- Reply to her email (in the same thread) confirming the refund and the amount.

Only touch that one order.
