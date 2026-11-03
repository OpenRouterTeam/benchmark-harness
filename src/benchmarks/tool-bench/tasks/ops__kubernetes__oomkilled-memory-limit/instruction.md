Something in the `analytics` namespace keeps getting OOM-killed overnight and the quarterly reports aren't going out. Figure out which workload it is and raise its container memory limit to 1Gi.

Only change the workload that is actually being killed, and leave staging alone.
