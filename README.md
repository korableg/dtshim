# dtshim

[English](README.en.md)

`dtshim` позволяет вызывать команды из Docker-контейнера и выполнять их на хосте.
Клиент в контейнере передаёт вызов демону на хосте и возвращает вывод и код завершения.
Для вызывающей программы это выглядит как запуск обычной команды.

Например, при автономной агентской разработке агент работает в изолированном
контейнере, где не всегда можно выполнить нужную команду: ей могут требоваться
доступ к корпоративной сети, системные возможности или инструменты, доступные
только на хосте. Через `dtshim` агент может вызвать такую команду из своего окружения.

Так, CLI для работы с внутренними сервисами можно оставить на хосте, где настроен
доступ к ним. Устанавливать этот инструмент и его зависимости в образ контейнера
не требуется.

## Как это работает

Пакет состоит из двух программ:

- `dtshimd` — демон на хосте. Принимает запросы через TCP или Unix-сокет и запускает процессы.
- `dtshim` — клиент внутри контейнера. Передаёт имя команды, аргументы, переменные
  окружения и стандартный ввод демону, а затем возвращает вывод и код завершения.

Определим для необходимых команд символическую ссылку на `dtshim`:

```sh
ln -s /usr/local/bin/dtshim /usr/bin/git
```

Вызов `git --version` через эту ссылку запускает `git --version` на хосте.

Транспорт выбирается аргументом `--transport tcp|unix` или переменной окружения `DTSHIM_TRANSPORT=tcp|unix`.
По умолчанию TCP и демон слушает `127.0.0.1:{20001-65535}`.
При запуске демон в stdout выводит tcp адрес или путь до unix сокета.
В режиме Unix без `--socket` и `DTSHIM_SOCKET_PATH` демон создаст сокет во временном каталоге.

**Сервер (`dtshimd`)**

| Настройка                             | Аргумент                         | Переменная окружения    |
| ------------------------------------- | -------------------------------- | ----------------------- |
| Транспорт (`tcp` или `unix`)          | `--transport <tcp/unix>`         | `DTSHIM_TRANSPORT`      |
| Путь Unix-сокета                      | `--socket <PATH>`                | `DTSHIM_SOCKET_PATH`    |
| Соответствия путей хоста и контейнера | `--volume_mapping <HOST:DOCKER>` | `DTSHIM_VOLUME_MAPPING` |
| Политика разрешённых команд           | `--policy <FILE>`                | `DTSHIM_POLICY`         |
| Токен подключения                     | `--token <TOKEN>`                | `DTSHIM_TOKEN`          |

Volume mapping позволяет подменять пути в `cwd`, `args`, `env`, `stdin`, `stdout` и `stderr`.

**Клиент (`dtshim`)**

Клиент тоже использует TCP по умолчанию. Его настройки задаются только
переменными окружения:

| Настройка                    | Аргумент | Переменная окружения |
| ---------------------------- | -------- | -------------------- |
| Транспорт (`tcp` или `unix`) | —        | `DTSHIM_TRANSPORT`   |
| TCP-адрес (`HOST:PORT`)      | —        | `DTSHIM_TCP_ADDR`    |
| Путь Unix-сокета             | —        | `DTSHIM_SOCKET_PATH` |
| Токен подключения            | —        | `DTSHIM_TOKEN`       |

Клиенту необходимо передать адрес или путь до сокета через env.

Токен необязателен: если он задан у демона, передайте тот же `DTSHIM_TOKEN` клиенту.
Клиент и демон обновляйте вместе.

Лимиты демона: 128 сессий, 10 секунд на первый полный запрос после принятия соединения.

```sh
DTSHIM_TRANSPORT=unix DTSHIM_SOCKET_PATH=/run/dtshim/socket git --version
DTSHIM_TRANSPORT=tcp DTSHIM_TCP_ADDR=host.docker.internal:20001 git --version
```

## Разрешённые команды и аргументы

[Пример политики](examples/policy.yaml):

```sh
dtshimd --policy examples/policy.yaml
```

Без политики команды не ограничены. С политикой вызов должен совпасть с одним
из вариантов `allow`; `executable` задаёт абсолютный путь на хосте независимо от `PATH` клиента.

В `env.host: [HOME, {LANG: "C.UTF-8"}]` имя сохраняет значение демона, а пара задаёт
фиксированное значение; клиент не может их переопределить.

`env.allow`: `null`/отсутствие — все переменные клиента, `[]` — никакие, список — только указанные.
Имена `env.host` должны входить в заданный список `env.allow`; окружение демона наследуется как прежде.

`value` и `positionals` проверяются regexp целиком; `required` делает опцию обязательной,
`repeat` разрешает повторы. Изменения политики применяются после перезапуска демона.

## Пример использования на Linux

В этом примере Docker Engine и демон работают на одном Linux-хосте, где уже
установлен Git.

### 1. Установить и запустить демон

Скачайте архив для своей платформы из [Releases](https://github.com/korableg/dtshim/releases).

| Платформа хоста     | Архив                                      |
| ------------------- | ------------------------------------------ |
| Linux x86_64        | `dtshim-x86_64-unknown-linux-musl.tar.gz`  |
| Linux ARM64         | `dtshim-aarch64-unknown-linux-musl.tar.gz` |
| macOS Apple Silicon | `dtshim-aarch64-apple-darwin.tar.gz`       |

Каждый архив содержит `dtshim` и `dtshimd`. Для Linux x86_64 установка демона
выглядит так:

```sh
tar -xzf dtshim-x86_64-unknown-linux-musl.tar.gz dtshimd
mkdir -p "$HOME/.local/bin"
install -m 755 dtshimd "$HOME/.local/bin/dtshimd"

mkdir -p "$HOME/.cache/dtshim"
chmod 700 "$HOME/.cache/dtshim"
"$HOME/.local/bin/dtshimd" --transport unix --socket "$HOME/.cache/dtshim/socket"
```

### 2. Добавить клиент в Docker-образ

Создайте Dockerfile:

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

Образ `ghcr.io/korableg/dtshim` содержит только статический Linux-клиент для `linux/amd64` и `linux/arm64`.

### 3. Подключить контейнер к демону

Передайте каталог с сокетом и укажите путь к нему внутри контейнера:

```sh
docker run --rm \
  --user "$(id -u):$(id -g)" \
  --mount "type=bind,src=$HOME/.cache/dtshim,dst=/run/dtshim" \
  --env DTSHIM_TRANSPORT=unix \
  --env DTSHIM_SOCKET_PATH=/run/dtshim/socket \
  dtshim-example git --version
```

Команда выведет версию Git, установленного на хосте. Контейнер запускается
с UID/GID пользователя демона, чтобы иметь доступ к сокету в закрытом каталоге.
`DTSHIM_SOCKET_PATH` обязателен для клиента в режиме Unix.

Для подключения по TCP на Linux запустите демон без аргументов:

```sh
"$HOME/.local/bin/dtshimd"
```

Адрес из stdout нужно передать как env `DTSHIM_TCP_ADDR`.

```sh
printf 'Адрес демона: '
read -r DTSHIM_TCP_ADDR
docker run --rm --network host \
  --env DTSHIM_TRANSPORT=tcp \
  --env DTSHIM_TCP_ADDR="$DTSHIM_TCP_ADDR" \
  dtshim-example git --version
```

Этот TCP-пример рассчитан на Docker Engine на Linux: сеть хоста позволяет
контейнеру подключиться к `127.0.0.1` демона.

## Тесты

```sh
cargo test --locked --workspace --lib --bins
cargo build --locked --workspace --bins
cargo test --locked -p integration
```
