#!/usr/bin/env bash
# Run the Buggregator update now, in the image the scheduled job uses, instead
# of waiting for the 03:37 run. The one-off job gets a unique name so it never
# collides with the schedule; it is removed after a successful run, and kept for
# inspection if the run fails or times out.
set -euo pipefail

JOB=buggregator-update
IMAGE=tool-magnustools/tool-magnustools:latest
NAME="buggregator-update-manual-$(date -u +%Y%m%dT%H%M%SZ)"
WAIT="${WAIT:-1800}"

echo "Running $JOB as $NAME (timeout ${WAIT}s)"

status=0
ssh login.toolforge.org "become magnustools toolforge-jobs run '$NAME' \
  --command 'job $JOB' --image '$IMAGE' --mount all --wait $WAIT" || status=$?

ssh login.toolforge.org "become magnustools toolforge-jobs logs '$NAME' --last 200" || true

if [ "$status" -eq 0 ]; then
	ssh login.toolforge.org "become magnustools toolforge-jobs delete '$NAME'" || true
else
	echo "Job $NAME did not finish successfully; keeping it for inspection." >&2
	echo "Delete it with: ssh login.toolforge.org \"become magnustools toolforge-jobs delete $NAME\"" >&2
fi
exit "$status"
