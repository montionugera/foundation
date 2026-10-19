#!/usr/bin/env bash
# Tests scripts/capped.sh. Each case starts a real scope, so it runs only on a box:
# Linux with cgroup v2 and a systemd user manager. No case uses more than 300 MiB, also
# when the cap does not hold.
#
# Usage: scripts/capped-test.sh

set -euo pipefail

script=$(cd "$(dirname "$0")" && pwd)/capped.sh
failed=0

# check <case> <status> <last line, a glob> <arg...>: runs capped.sh with the args.
check() {
    local name=$1 want_status=$2 want_line=$3
    shift 3
    local out status=0
    out=$("$script" "$@" 2>&1) || status=$?
    local line=${out##*$'\n'}
    # shellcheck disable=SC2053 # want_line is a glob.
    if [[ $status == "$want_status" && $line == $want_line ]]; then
        echo "ok $name"
    else
        printf 'FAIL %s: status %s, last line %q\n' "$name" "$status" "$line"
        failed=1
    fi
}

check pass 0 'capped: exit 0, max 0, oom_kill 0' 256M true
check status 3 'capped: exit 3, max 0, oom_kill 0' 256M sh -c 'exit 3'
check env 0 'capped: exit 0, max 0, oom_kill 0' \
    256M sh -c 'test "$CARGO_BUILD_JOBS $RUST_TEST_THREADS" = "4 4"'
# `tail` holds the whole input when it has no newline.
check fits 0 'capped: exit 0, max 0, oom_kill 0' \
    256M sh -c 'head -c 100M /dev/zero | tail >/dev/null'
check over 137 'capped: exit 137, max [1-9]*, oom_kill 1' \
    256M sh -c 'head -c 300M /dev/zero | tail >/dev/null'
check usage 2 'usage: scripts/capped.sh <cap> <command> [arg...]' 256M

exit "$failed"
