QA wants to test gift cards on storefront preview deployments before we launch them. Please set up these environment variables on our `storefront` project in Vercel:

- `NEXT_PUBLIC_ENABLE_GIFT_CARDS` = `true` for Preview and Development. Production must not get it yet.
- `PAYMENTS_API_URL` = `https://sandbox.pay.acme-shop.com` for Preview only, so preview builds hit the payments sandbox. Production already has its own `PAYMENTS_API_URL`, and it has to stay exactly as it is.

Don't remove any existing variables.
