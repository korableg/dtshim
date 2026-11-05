#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

cargo build --locked -p client --bin dtshim
cargo build --locked -p daemon --bin dtshimd

timeout --kill-after=2s 10s sh -eu <<'SH'
temporary=$(mktemp -d /tmp/dtshim-test.XXXXXX)
server_pid=
trap '
    [ -z "$server_pid" ] || kill "$server_pid" 2>/dev/null || :
    [ -z "$server_pid" ] || wait "$server_pid" 2>/dev/null || :
    rm -rf "$temporary"
' 0
trap 'exit 143' TERM

mkfifo "$temporary/ready"
target/debug/dtshimd --socket "$temporary/socket" >"$temporary/ready" 2>"$temporary/server.stderr" &
server_pid=$!
IFS= read -r DTSHIM_SOCKET_PATH < "$temporary/ready"
export DTSHIM_SOCKET_PATH
ln -s "$PWD/target/debug/dtshim" "$temporary/cat"

printf 'hello through stdin\n' > "$temporary/input"
status=0
"$temporary/cat" - "$temporary/missing" <"$temporary/input" >"$temporary/output" 2>"$temporary/error" || status=$?
[ "$status" -eq 1 ]
cmp "$temporary/input" "$temporary/output"
grep -F "$temporary/missing" "$temporary/error" >/dev/null
printf 'Integration test passed: stdin, stdout, stderr, and exit status\n'
SH
