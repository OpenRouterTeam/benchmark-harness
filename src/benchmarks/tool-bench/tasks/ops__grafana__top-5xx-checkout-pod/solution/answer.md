In production (Prometheus (prod), namespace shop-prod) the checkout pod with the highest 5xx rate is
**checkout-7b9d6c5f4-xk2lp** at about 3.5 errors/s (2.9/s HTTP 500 plus 0.6/s HTTP 502).
The other two production pods are at roughly 0.22-0.26 errors/s.
