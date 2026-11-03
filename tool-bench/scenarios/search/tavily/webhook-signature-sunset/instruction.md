Our payments integration still verifies Ferrowave webhooks with the old v1 signature. According to Ferrowave's current developer docs (docs.ferrowave.dev), on what date will Ferrowave stop sending v1 webhook signatures, and what header do we have to verify instead? Several deprecation dates on that site look alike, so make sure you have the one for webhook signatures.

Write the date and the replacement header name to `/app/answer.md`.
