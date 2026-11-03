Production for `storefront` is served by deployment dpl_8o704GjcwumkLAaxPwjoxb4V6QDF, built from
commit c3a9f12e8b47d05a6e21f9b3c7d84e50a1b6f2d9 ("Fix tax rounding for EU orders (#482)", Mateo Alvarez).
The newer production deployment for "Upgrade to Next.js 15.4 (#487)" (7d41c9e) failed its build, so it never went live.

The latest preview deployment (branch feat/gift-cards, commit 4be1d07) failed because a Serverless Function
exceeded the unzipped maximum size of 250 MB (FUNCTION_SIZE_EXCEEDED).
