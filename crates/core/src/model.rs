use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Start {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    Start(Start),
    Stdin(#[serde(with = "serde_bytes")] Vec<u8>),
    Stdout(#[serde(with = "serde_bytes")] Vec<u8>),
    Stderr(#[serde(with = "serde_bytes")] Vec<u8>),
    Code(i32),
    Signaled(i32),
    Failed(String),
}
