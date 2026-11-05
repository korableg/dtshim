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

- `dtshimd` — the host daemon. Accepts requests through a Unix socket and starts processes.
- `dtshim` — the client inside the container. Sends the command name, arguments,
  environment variables, and standard input to the daemon, then returns the output
  and exit code.

Create a symbolic link to `dtshim` for each command you want to forward:

```sh
ln -s /usr/local/bin/dtshim /usr/bin/git
```

Calling `git --version` through this link runs `git --version` on the host.
The syntax `dtshim git --version` is not used to select the command.

## Usage example on Linux

In this example, Docker Engine and the daemon run on the same Linux host,
which already has Git installed.

### 1. Install and start the daemon

Download the archive for your platform from **Actions → Test and publish builds →
a successful run → Artifacts** in this repository.

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
"$HOME/.local/bin/dtshimd" --socket "$HOME/.cache/dtshim/socket"
```

Leave the daemon running in this terminal. It prints the socket path.
If `--socket` is omitted, the daemon creates a socket in a temporary directory.

### 2. Add the client to your Docker image

Create a Dockerfile:

```dockerfile
FROM debian:bookworm-slim

COPY --from=ghcr.io/korableg/dtshim:latest /dtshim /usr/local/bin/dtshim

RUN mkdir -p /opt/host-bin \
    && ln -s /usr/local/bin/dtshim /opt/host-bin/git

ENV PATH="/opt/host-bin:${PATH}"
```

The `/opt/host-bin` directory comes first in `PATH`, so calls to `git` go through
the client. Add similar links for other commands.

Build the image in another terminal:

```sh
docker build -t dtshim-example .
```

The `ghcr.io/korableg/dtshim` image contains only the statically linked Linux client
for `linux/amd64` and `linux/arm64`. Docker selects the appropriate architecture.
For reproducible builds, replace `latest` with a published tag, such as `v0.1.0`,
or an image digest.

### 3. Connect the container to the daemon

Mount the socket directory and specify the socket path inside the container:

```sh
docker run --rm \
  --user "$(id -u):$(id -g)" \
  --mount "type=bind,src=$HOME/.cache/dtshim,dst=/run/dtshim" \
  --env DTSHIM_SOCKET_PATH=/run/dtshim/socket \
  dtshim-example git --version
```

The command prints the version of Git installed on the host. The container runs
with the daemon user's UID/GID so it can access the socket in the private directory.
`DTSHIM_SOCKET_PATH` is required by the client.

Standard streams are forwarded between the container and the host process.
To pass stdin through `docker run`, add the `-i` flag. The command's exit code
is returned to the calling process inside the container.
