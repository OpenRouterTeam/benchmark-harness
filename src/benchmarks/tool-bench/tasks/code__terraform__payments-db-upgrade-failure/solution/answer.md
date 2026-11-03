Latest run on payments-api-prod: run-Gy4tBHGggKx49nbH ("Upgrade payments DB to Postgres 16"), status errored.

The plan succeeded (Plan: 1 to add, 1 to change, 0 to destroy); the apply failed on aws_db_instance.payments
after creating aws_db_parameter_group.payments_pg16:

  api error InvalidParameterCombination: Cannot upgrade postgres from 14.12 to 16.4. You must set
  AllowMajorVersionUpgrade to true when upgrading to a new major version.
