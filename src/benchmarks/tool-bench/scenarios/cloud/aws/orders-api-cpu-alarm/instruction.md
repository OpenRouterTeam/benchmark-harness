Our production orders-api server in AWS has been running hot this morning and nobody got paged. Please set up a CloudWatch alarm named `orders-api-high-cpu` on that instance that fires when its average CPU utilization is greater than 80% for three consecutive 5-minute periods. It should notify the same `ops-pager` SNS topic our other EC2 alarms use.

Don't change or delete any of the existing alarms.
