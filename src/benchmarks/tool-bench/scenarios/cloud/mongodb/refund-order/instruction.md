Support just approved a full refund for Priya Natarajan's order #10482 — the sleeping bag arrived damaged. In our production MongoDB `shop` database, mark that order as refunded: set its status to `refunded` and record the refund on the order as `refund.reason` = "damaged in transit" and `refund.amount` = the order's total (as a number).

Don't touch any other orders.
