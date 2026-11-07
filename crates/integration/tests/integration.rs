#![cfg(unix)]

use std::{fs, os::unix::process::CommandExt, process::Stdio, time::Duration};

use assert_cmd::{
    Command,
    cargo::{CommandCargoExt, cargo_bin},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    time::timeout,
};

const TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn unix() {
    let directory = tempfile::tempdir().unwrap();
    let socket = directory.path().join("socket");
    let mut daemon = tokio::process::Command::new(cargo_bin("dtshimd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args(["--transport", "unix", "--socket"])
        .arg(&socket)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(daemon.stdout.take().unwrap()).lines();
    let address = timeout(TIMEOUT, stdout.next_line())
        .await
        .expect("daemon readiness timed out")
        .unwrap()
        .expect("daemon exited before announcing its address");
    assert_eq!(address, socket.to_str().unwrap());

    let mut client = std::process::Command::cargo_bin("dtshim").unwrap();
    client.arg0("sh");
    Command::from_std(client)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("DTSHIM_TRANSPORT", "unix")
        .env("DTSHIM_SOCKET_PATH", address)
        .env("DTSHIM_TEST_ENV", "env value")
        .arg("-c")
        .arg(r#"read -r input; printf '%s|%s|%s\n' "$input" "$1" "$DTSHIM_TEST_ENV"; printf 'stderr\n' >&2; exit 7"#)
        .arg("sh")
        .arg("arg with spaces")
        .write_stdin("stdin\n")
        .timeout(TIMEOUT)
        .assert()
        .code(7)
        .stdout("stdin|arg with spaces|env value\n")
        .stderr("stderr\n");

    daemon.kill().await.unwrap();
}

#[tokio::test]
async fn tcp() {
    let mut daemon = tokio::process::Command::new(cargo_bin("dtshimd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args(["--transport", "tcp"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(daemon.stdout.take().unwrap()).lines();
    let address = timeout(TIMEOUT, stdout.next_line())
        .await
        .expect("daemon readiness timed out")
        .unwrap()
        .expect("daemon exited before announcing its address");

    let mut client = std::process::Command::cargo_bin("dtshim").unwrap();
    client.arg0("sh");
    Command::from_std(client)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("DTSHIM_TRANSPORT", "tcp")
        .env("DTSHIM_TCP_ADDR", address)
        .env("DTSHIM_TEST_ENV", "env value")
        .arg("-c")
        .arg(r#"read -r input; printf '%s|%s|%s\n' "$input" "$1" "$DTSHIM_TEST_ENV"; printf 'stderr\n' >&2; exit 7"#)
        .arg("sh")
        .arg("arg with spaces")
        .write_stdin("stdin\n")
        .timeout(TIMEOUT)
        .assert()
        .code(7)
        .stdout("stdin|arg with spaces|env value\n")
        .stderr("stderr\n");

    daemon.kill().await.unwrap();
}

#[tokio::test]
async fn policy() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let policy = directory.path().join("policy.yaml");
    fs::write(
        &policy,
        "commands:\n  dtshim:\n    executable: /bin/echo\n    allow:\n      - prefix: [allowed]\n",
    )
    .unwrap();
    let mut daemon = tokio::process::Command::new(cargo_bin("dtshimd"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args(["--transport", "tcp", "--policy"])
        .arg(policy)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(daemon.stdout.take().unwrap()).lines();
    let address = timeout(TIMEOUT, stdout.next_line())
        .await
        .expect("daemon readiness timed out")
        .unwrap()
        .expect("daemon exited before announcing its address");

    Command::cargo_bin("dtshim")
        .unwrap()
        .arg("allowed")
        .env_clear()
        .env("DTSHIM_TRANSPORT", "tcp")
        .env("DTSHIM_TCP_ADDR", &address)
        .timeout(TIMEOUT)
        .assert()
        .success()
        .stdout("allowed\n")
        .stderr("");

    Command::cargo_bin("dtshim")
        .unwrap()
        .arg("denied")
        .env_clear()
        .env("DTSHIM_TRANSPORT", "tcp")
        .env("DTSHIM_TCP_ADDR", &address)
        .timeout(TIMEOUT)
        .assert()
        .code(1)
        .stdout("")
        .stderr("Error: command failed: policy denied: arguments for command \"dtshim\" do not match any allowed invocation\n");

    let mut client = std::process::Command::cargo_bin("dtshim").unwrap();
    client.arg0("unknown");
    Command::from_std(client)
        .env_clear()
        .env("DTSHIM_TRANSPORT", "tcp")
        .env("DTSHIM_TCP_ADDR", &address)
        .timeout(TIMEOUT)
        .assert()
        .code(1)
        .stdout("")
        .stderr("Error: command failed: policy denied: command \"unknown\" is not allowed\n");

    daemon.kill().await.unwrap();
}
