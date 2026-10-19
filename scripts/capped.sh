#!/usr/bin/env bash
# Runs a command on a box in a cgroup of its own, with a memory cap, no swap, four build
# jobs, and four test threads. After the command ends, it prints the memory events of
# the cgroup, and exits with the status of the command.
#
# Usage: scripts/capped.sh <cap> <command> [arg...]
#
#   cap      the memory cap, such as 8G
#   command  the command to run, such as cargo test -p node
#
# Its last line is `capped: exit <status>, max <n>, oom_kill <n>`: the times that the
# cgroup reached its cap, and the processes in it that the kernel killed for lack of
# memory. docs/claude/testing.md, "Runs on a box", says how to read it.
#
# Needs Linux with cgroup v2 and a systemd user manager with the memory controller.

set -euo pipefail

if [[ ${1:-} == --in-scope ]]; then
    scope=/sys/fs/cgroup$(cut -d: -f3- /proc/self/cgroup)
    run=$scope/run
    # The report runs outside the cap, where a kill at the cap cannot reach it.
    mkdir "$scope/report"
    echo $$ >"$scope/report/cgroup.procs"
    echo +memory >"$scope/cgroup.subtree_control"
    mkdir "$run"
    echo "$2" >"$run/memory.max"
    echo 0 >"$run/memory.swap.max"
    shift 2
    status=0
    (echo "$BASHPID" >"$run/cgroup.procs" && exec "$@") || status=$?
    count() { awk -v key="$1" '$1 == key { print $2 }' "$run/memory.events"; }
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
# systemd stops a scope at a kill for lack of memory in it, unless its OOMPolicy is
# continue.
exec systemd-run --user --scope --quiet -p Delegate=memory -p OOMPolicy=continue \
    env CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=4 bash "$0" --in-scope "$cap" "$@"
