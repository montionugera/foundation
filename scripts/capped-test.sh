#!/usr/bin/env bash
# Tests scripts/capped.sh. Each case starts a real scope, so it runs only on a box:
# Linux with cgroup v2 and a systemd user manager. No case uses more than 300 MiB, also
# when the cap does not hold. The case `cache` writes 512 MiB to a file in /var/tmp.
#
# Usage: scripts/capped-test.sh

set -euo pipefail

script=$(cd "$(dirname "$0")" && pwd)/capped.sh
failed=0
clean='capped: exit 0, oom 0, oom_kill 0, sock_throttled 0'

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

# own <cap> <file> <value>: in the cap, the file of the cgroup of the command holds the
# value.
own() {
    check "$2" 0 "$clean" "$1" sh -c \
        'test "$(cat "/sys/fs/cgroup$(cut -d: -f3- /proc/self/cgroup)/$0")" = "$1"' \
        "$2" "$3"
}

check pass 0 "$clean" 256M true
check status 3 'capped: exit 3, oom 0, oom_kill 0, sock_throttled 0' 256M sh -c 'exit 3'
check env 0 "$clean" \
    256M sh -c 'test "$CARGO_BUILD_JOBS $RUST_TEST_THREADS" = "4 4"'
# The parent of the command is the report.
check apart 0 "$clean" \
    256M sh -c 'test "$(cat /proc/self/cgroup)" != "$(cat "/proc/$PPID/cgroup")"'
own 256M memory.swap.max 0
own 0.25G memory.max 268435456
check policy 0 "$clean" 256M sh -c 'scope=$(dirname "$(cut -d: -f3- /proc/self/cgroup)")
    test "$(systemctl --user show -P OOMPolicy "${scope##*/}")" = continue'
check fits 0 "$clean" 256M dd if=/dev/zero of=/dev/null bs=100M count=1 status=none
# The kernel frees page cache at the cap with no count.
file=$(mktemp /var/tmp/capped-test.XXXXXX)
trap 'rm -f "$file"' EXIT
check cache 0 "$clean" 256M dd if=/dev/zero of="$file" bs=1M count=512 status=none
# Eight runs: a report in the cap lost its line in 3 of 8. One process holds the memory:
# with a pipeline, the kernel killed two processes in 7 of 16 runs.
for i in 1 2 3 4 5 6 7 8; do
    check "over $i" 137 'capped: exit 137, oom [1-9]*, oom_kill 1, sock_throttled 0' \
        256M dd if=/dev/zero of=/dev/null bs=300M count=1 status=none
    # The kill takes one process: the shell lives on.
    check "alone $i" 0 'capped: exit 0, oom [1-9]*, oom_kill 1, sock_throttled 0' \
        256M sh -c 'dd if=/dev/zero of=/dev/null bs=300M count=1 status=none; true'
done
check usage 2 'usage: scripts/capped.sh <cap> <command> \[arg...\]' 256M

exit "$failed"
