#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

cargo build --locked -p client --bin dtshim
cargo build --locked -p daemon --bin dtshimd

timeout --kill-after=2s 10s sh -eu <<'SH'
temporary=$(mktemp -d /tmp/dtshim-unix-test.XXXXXX)
server_pid=
trap '
    [ -z "$server_pid" ] || kill "$server_pid" 2>/dev/null || :
    [ -z "$server_pid" ] || wait "$server_pid" 2>/dev/null || :
    rm -rf "$temporary"
' 0
trap 'exit 143' TERM

unset DTSHIM_TRANSPORT DTSHIM_TCP_ADDR DTSHIM_SOCKET_PATH DTSHIM_TEST_ENV
mkfifo "$temporary/ready"
target/debug/dtshimd --transport unix --socket "$temporary/socket" >"$temporary/ready" 2>"$temporary/server.stderr" &
server_pid=$!
IFS= read -r address < "$temporary/ready"
[ "$address" = "$temporary/socket" ]
[ -S "$address" ]
export DTSHIM_TRANSPORT=unix DTSHIM_SOCKET_PATH="$address"

ln -s "$PWD/target/debug/dtshim" "$temporary/cat"
ln -s "$PWD/target/debug/dtshim" "$temporary/printf"
ln -s "$PWD/target/debug/dtshim" "$temporary/printenv"

printf 'hello through stdin\n' > "$temporary/input"
status=0
"$temporary/cat" - "$temporary/missing" <"$temporary/input" >"$temporary/output" 2>"$temporary/error" || status=$?
[ "$status" -eq 1 ]
cmp "$temporary/input" "$temporary/output"
grep -F "$temporary/missing" "$temporary/error" >/dev/null

printf '%s\n' --help --version --transport --dtshim-transport=unix -- --dtshim-unknown '' >"$temporary/expected-args"
"$temporary/printf" '%s\n' --help --version --transport --dtshim-transport=unix -- --dtshim-unknown '' \
    </dev/null >"$temporary/actual-args"
cmp "$temporary/expected-args" "$temporary/actual-args"

export DTSHIM_TEST_ENV='forwarded value with spaces'
printf '%s\n' "$DTSHIM_TEST_ENV" >"$temporary/expected-env"
"$temporary/printenv" DTSHIM_TEST_ENV </dev/null >"$temporary/actual-env"
cmp "$temporary/expected-env" "$temporary/actual-env"

kill "$server_pid"
wait "$server_pid"
server_pid=
[ ! -e "$temporary/socket" ]
printf 'Unix integration test passed: streams, exit status, arguments, environment, and socket cleanup\n'
SH
