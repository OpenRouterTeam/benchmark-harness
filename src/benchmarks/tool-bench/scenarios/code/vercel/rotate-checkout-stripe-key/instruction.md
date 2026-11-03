Our live Stripe secret key for the checkout API got pasted into a support ticket, so I've rolled it in Stripe. The new key is `sk_live_51NvCheckoutRot8Hq2`.

Please swap it in on Vercel: replace the production `STRIPE_SECRET_KEY` of the checkout project that serves `api.acme-shop.com` with the new key, and keep it a sensitive variable. The preview/development test key on that project stays as it is. Don't touch any other project.
