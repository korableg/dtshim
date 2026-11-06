#!/bin/sh
set -eu
cd "$(dirname "$0")/.."

cargo build --locked -p client --bin dtshim
cargo build --locked -p daemon --bin dtshimd

timeout --kill-after=2s 10s sh -eu <<'SH'
temporary=$(mktemp -d /tmp/dtshim-tcp-test.XXXXXX)
server_pid=
trap '
    [ -z "$server_pid" ] || kill "$server_pid" 2>/dev/null || :
    [ -z "$server_pid" ] || wait "$server_pid" 2>/dev/null || :
    rm -rf "$temporary"
' 0
trap 'exit 143' TERM

unset DTSHIM_TRANSPORT DTSHIM_TCP_ADDR DTSHIM_SOCKET_PATH DTSHIM_TEST_ENV
unset DTSHIM_HOST_ENV DTSHIM_VOLUME_MAPPING DTSHIM_TEST_MISSING
mkfifo "$temporary/ready"
target/debug/dtshimd >"$temporary/ready" 2>"$temporary/server.stderr" &
server_pid=$!
IFS= read -r address < "$temporary/ready"
[ "${address%:*}" = 127.0.0.1 ]
port=${address##*:}
[ "$port" -gt 20000 ]
[ "$port" -le 65535 ]
export DTSHIM_TCP_ADDR="$address"

ln -s "$PWD/target/debug/dtshim" "$temporary/cat"
ln -s "$PWD/target/debug/dtshim" "$temporary/printf"
ln -s "$PWD/target/debug/dtshim" "$temporary/printenv"
ln -s "$PWD/target/debug/dtshim" "$temporary/sh"

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

for mode in repeated comma env precedence; do
    unset DTSHIM_HOST_ENV
    case "$mode" in
        repeated)
            set -- --host_env PATH --host_env DTSHIM_TEST_HOST_ENV --host_env DTSHIM_TEST_MISSING
            ;;
        comma)
            set -- --host_env PATH,DTSHIM_TEST_HOST_ENV,DTSHIM_TEST_MISSING
            ;;
        env)
            export DTSHIM_HOST_ENV=PATH,DTSHIM_TEST_HOST_ENV,DTSHIM_TEST_MISSING
            set --
            ;;
        precedence)
            export DTSHIM_HOST_ENV=DTSHIM_TEST_ENV
            set -- --host_env PATH,DTSHIM_TEST_HOST_ENV,DTSHIM_TEST_MISSING
            ;;
    esac

    PATH=/usr/bin:/bin DTSHIM_TEST_HOST_ENV=/container/kept DTSHIM_TEST_ENV=host \
        target/debug/dtshimd --volume_mapping /host:/container "$@" \
        >"$temporary/ready" 2>"$temporary/server.stderr" &
    server_pid=$!
    IFS= read -r address < "$temporary/ready"
    export DTSHIM_TCP_ADDR="$address"

    {
        printf '%s\n' /usr/bin:/bin client
        printf '%s' /container/kept | cksum
        printf '%s' /host/client | cksum
    } >"$temporary/expected-host-env"

    PATH=/nonexistent DTSHIM_TEST_HOST_ENV=/container/client DTSHIM_TEST_ENV=client \
        DTSHIM_TEST_MISSING=client DTSHIM_TEST_MAPPED_ENV=/container/client \
        "$temporary/sh" -c '
            printf "%s\n" "$PATH" "$DTSHIM_TEST_ENV"
            printf "%s" "$DTSHIM_TEST_HOST_ENV" | cksum
            printf "%s" "$DTSHIM_TEST_MAPPED_ENV" | cksum
            test "${DTSHIM_TEST_MISSING+x}" != x
        ' </dev/null >"$temporary/actual-host-env"
    cmp "$temporary/expected-host-env" "$temporary/actual-host-env"

    kill "$server_pid"
    wait "$server_pid"
    server_pid=
done

printf 'TCP integration test passed: default transport, streams, exit status, arguments, environment, and host environment selection\n'
SH
