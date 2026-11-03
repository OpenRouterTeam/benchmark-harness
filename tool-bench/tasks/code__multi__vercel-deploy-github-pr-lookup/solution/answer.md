Production for `storefront` on Vercel is served by deployment dpl_8o704GjcwumkLAaxPwjoxb4V6QDF, commit
c3a9f12e8b47d05a6e21f9b3c7d84e50a1b6f2d9 ("Fix tax rounding for EU orders (#482)"). The newer production
deployment of 7d41c9e (Upgrade to Next.js 15.4, #487) failed, so it never went live.

That commit is the squash merge of acme-shop/storefront PR #482 "Fix tax rounding for EU orders"
(EU VAT rounded half-even per line item; it supersedes the unmerged #478):

- Author: mateoalv (Mateo Alvarez)
- Approved by: ines-d (Ines Duarte); junpark only left a comment
- Merged by: riley-p (Riley Park) on 2026-10-24 09:38 UTC
