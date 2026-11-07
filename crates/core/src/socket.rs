use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Unix,
}

impl FromStr for Transport {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "tcp" => Ok(Self::Tcp),
            "unix" => Ok(Self::Unix),
            _ => Err("transport must be tcp or unix"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_supported_transport_names() {
        assert_eq!("tcp".parse(), Ok(Transport::Tcp));
        assert_eq!("unix".parse(), Ok(Transport::Unix));
        for value in ["", "TCP", "Unix", "udp", " tcp", "unix\n"] {
            assert_eq!(
                value.parse::<Transport>(),
                Err("transport must be tcp or unix"),
                "{value:?}"
            );
        }
    }
}
