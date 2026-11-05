# dtshim

[Русский](README.md)

`dtshim` lets you invoke commands from a Docker container and execute them on the host.
The client inside the container forwards the invocation to the host daemon and
returns the output and exit code. To the calling program, it looks like running
a regular command.

For example, in autonomous agent development, an agent runs in an isolated
container where some commands cannot be executed: they may require access to
a corporate network, system capabilities, or tools available only on the host.
With `dtshim`, the agent can invoke these commands from its own environment.

A CLI for internal services can stay on the host, where access to those services
is already configured. There is no need to install the tool or its dependencies
in the container image.

## How it works

The package consists of two programs:

- `dtshimd` — the host daemon. Accepts requests through TCP or a Unix socket and starts processes.
- `dtshim` — the client inside the container. Sends the command name, arguments,
  environment variables, and standard input to the daemon, then returns the output
  and exit code.

Create a symbolic link to `dtshim` for each command you want to forward:

```sh
ln -s /usr/local/bin/dtshim /usr/bin/git
```

Calling `git --version` through this link runs `git --version` on the host.

Select the transport with `--transport tcp|unix` or the
`DTSHIM_TRANSPORT=tcp|unix` environment variable.
TCP is the default, and the daemon listens on `127.0.0.1:{20001-65535}`.
At startup, the daemon writes the TCP address or Unix socket path to stdout.
In Unix mode, if both `--socket` and `DTSHIM_SOCKET_PATH` are omitted, the daemon
creates a socket in a temporary directory.

**Server (`dtshimd`)**

| Setting | Argument | Environment variable |
| --- | --- | --- |
| Transport (`tcp` or `unix`) | `--transport <tcp/unix>` | `DTSHIM_TRANSPORT` |
| Unix socket path | `--socket <PATH>` | `DTSHIM_SOCKET_PATH` |

**Client (`dtshim`)**

The client also defaults to TCP. Configure it only through environment variables:

| Setting | Argument | Environment variable |
| --- | --- | --- |
| Transport (`tcp` or `unix`) | — | `DTSHIM_TRANSPORT` |
| TCP address (`HOST:PORT`) | — | `DTSHIM_TCP_ADDR` |
| Unix socket path | — | `DTSHIM_SOCKET_PATH` |

The client requires the address or socket path to be passed through environment variables.

```sh
DTSHIM_TRANSPORT=unix DTSHIM_SOCKET_PATH=/run/dtshim/socket git --version
DTSHIM_TRANSPORT=tcp DTSHIM_TCP_ADDR=host.docker.internal:20001 git --version
```

## Usage example on Linux

In this example, Docker Engine and the daemon run on the same Linux host,
which already has Git installed.

### 1. Install and start the daemon

Download the archive for your platform from [Releases](https://github.com/korableg/dtshim/releases).

| Host platform | Archive |
| --- | --- |
| Linux x86_64 | `dtshim-x86_64-unknown-linux-musl.tar.gz` |
| Linux ARM64 | `dtshim-aarch64-unknown-linux-musl.tar.gz` |
| macOS Apple Silicon | `dtshim-aarch64-apple-darwin.tar.gz` |

Each archive contains `dtshim` and `dtshimd`. To install the daemon on Linux x86_64:

```sh
tar -xzf dtshim-x86_64-unknown-linux-musl.tar.gz dtshimd
mkdir -p "$HOME/.local/bin"
install -m 755 dtshimd "$HOME/.local/bin/dtshimd"

mkdir -p "$HOME/.cache/dtshim"
chmod 700 "$HOME/.cache/dtshim"
"$HOME/.local/bin/dtshimd" --transport unix --socket "$HOME/.cache/dtshim/socket"
```

### 2. Add the client to your Docker image

Create a Dockerfile:

```dockerfile
FROM debian:bookworm-slim

COPY --from=ghcr.io/korableg/dtshim:latest /dtshim /usr/local/bin/dtshim

RUN mkdir -p /opt/host-bin \
    && ln -s /usr/local/bin/dtshim /opt/host-bin/git

ENV PATH="/opt/host-bin:${PATH}"
```

```sh
docker build -t dtshim-example .
```

The `ghcr.io/korableg/dtshim` image contains only the statically linked Linux client
for `linux/amd64` and `linux/arm64`.

### 3. Connect the container to the daemon

Mount the socket directory and specify the socket path inside the container:

```sh
docker run --rm \
  --user "$(id -u):$(id -g)" \
  --mount "type=bind,src=$HOME/.cache/dtshim,dst=/run/dtshim" \
  --env DTSHIM_TRANSPORT=unix \
  --env DTSHIM_SOCKET_PATH=/run/dtshim/socket \
  dtshim-example git --version
```

The command prints the version of Git installed on the host. The container runs
with the daemon user's UID/GID so it can access the socket in the private directory.
`DTSHIM_SOCKET_PATH` is required by the client in Unix mode.

To connect over TCP on Linux, start the daemon without arguments:

```sh
"$HOME/.local/bin/dtshimd"
```

Pass the address printed to stdout through the `DTSHIM_TCP_ADDR` environment variable.

```sh
printf 'Daemon address: '
read -r DTSHIM_TCP_ADDR
docker run --rm --network host \
  --env DTSHIM_TRANSPORT=tcp \
  --env DTSHIM_TCP_ADDR="$DTSHIM_TCP_ADDR" \
  dtshim-example git --version
```

This TCP example targets Docker Engine on Linux: host networking lets the
container connect to the daemon's `127.0.0.1` address.
