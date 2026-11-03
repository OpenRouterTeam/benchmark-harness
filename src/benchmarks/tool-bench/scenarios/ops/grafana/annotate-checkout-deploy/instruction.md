We shipped checkout v4.19.0 to production today at 14:05 UTC (12 Nov 2026), and I want the latency graphs to show it. In Grafana, add a single annotation on the production Checkout Service dashboard at exactly that time with the text "Deploy checkout v4.19.0", tagged `deploy` and `checkout`.

Don't edit the dashboard itself, and leave the staging and legacy checkout dashboards alone.
