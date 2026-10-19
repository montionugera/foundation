#!/usr/bin/env bash
# Runs a command on a box in a systemd scope of its own, with a memory cap, no swap,
# four build jobs, and four test threads. After the command ends, it prints the memory
# events of the scope, and exits with the status of the command.
#
# Usage: scripts/capped.sh <cap> <command> [arg...]
#
#   cap      the MemoryMax of the scope, such as 8G
#   command  the command to run, such as cargo test -p node
#
# Its last line is `capped: exit <status>, max <n>, oom_kill <n>`: the times that the
# scope reached its cap, and the processes of the scope that the kernel killed for
# lack of memory. docs/claude/testing.md, "Runs on a box", says how to read it.
#
# Needs Linux with cgroup v2 and a systemd user manager.

set -euo pipefail

if [[ ${1:-} == --in-scope ]]; then
    shift
    status=0
    "$@" || status=$?
    events=/sys/fs/cgroup$(cut -d: -f3- /proc/self/cgroup)/memory.events
    count() { awk -v key="$1" '$1 == key { print $2 }' "$events"; }
    max=$(count max)
    kills=$(count oom_kill)
    echo "capped: exit $status, max $max, oom_kill $kills"
    exit "$status"
fi

if (($# < 2)); then
    echo "usage: scripts/capped.sh <cap> <command> [arg...]" >&2
    exit 2
fi
cap=$1
shift
# The default OOMPolicy of a user manager stops the whole scope at a kill for lack of
# memory, and then no process is left to print the counts.
exec systemd-run --user --scope --quiet -p MemoryMax="$cap" -p MemorySwapMax=0 \
    -p OOMPolicy=continue env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 \
    bash "$0" --in-scope "$@"
