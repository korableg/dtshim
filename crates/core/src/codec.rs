use std::io;

use tokio_util::{
    bytes::{Bytes, BytesMut},
    codec::{Decoder, Encoder, LengthDelimitedCodec},
};

use crate::model::Message;

pub struct PostcardCodec(LengthDelimitedCodec);

impl PostcardCodec {
    pub fn new() -> Self {
        Self(LengthDelimitedCodec::builder().new_codec())
    }
}

impl Decoder for PostcardCodec {
    type Item = Message;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Message>, Self::Error> {
        match self.0.decode(src)? {
            Some(frame) => match postcard::from_bytes(&frame) {
                Ok(msg) => Ok(Some(msg)),
                Err(err) => Err(io::Error::new(io::ErrorKind::InvalidData, err)),
            },
            None => Ok(None),
        }
    }
}

impl Encoder<Message> for PostcardCodec {
    type Error = io::Error;

    fn encode(&mut self, item: Message, dst: &mut BytesMut) -> Result<(), Self::Error> {
        match postcard::to_allocvec(&item) {
            Ok(val) => self.0.encode(Bytes::from(val), dst),
            Err(err) => Err(io::Error::new(io::ErrorKind::InvalidData, err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::model::Start;

    use super::*;

    fn frame(message: Message) -> BytesMut {
        let mut bytes = BytesMut::new();
        PostcardCodec::new().encode(message, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn start_preserves_token_arguments_environment_and_working_directory() {
        for token in [None, Some("secret"), Some("")] {
            let mut bytes = frame(Message::Start(Start {
                token: token.map(str::to_owned),
                command: "команда".into(),
                args: vec!["".into(), "argument with spaces".into()],
                env: vec![("NAME".into(), "value\nwith\0bytes".into())],
                cwd: "/work dir".into(),
            }));
            let Some(Message::Start(start)) = PostcardCodec::new().decode(&mut bytes).unwrap()
            else {
                panic!("expected start message");
            };
            assert_eq!(start.token.as_deref(), token);
            assert_eq!(start.command, "команда");
            assert_eq!(start.args, ["", "argument with spaces"]);
            assert_eq!(
                start.env,
                [("NAME".to_owned(), "value\nwith\0bytes".to_owned())]
            );
            assert_eq!(start.cwd, "/work dir");
            assert!(bytes.is_empty());
        }
    }

    #[test]
    fn stream_messages_preserve_empty_and_non_utf8_payloads() {
        for payload in [vec![], vec![0, 255, 128, b'\n']] {
            for message in [
                Message::Stdin(payload.clone()),
                Message::Stdout(payload.clone()),
                Message::Stderr(payload.clone()),
            ] {
                let kind = std::mem::discriminant(&message);
                let decoded = PostcardCodec::new()
                    .decode(&mut frame(message))
                    .unwrap()
                    .unwrap();
                assert_eq!(std::mem::discriminant(&decoded), kind);
                match decoded {
                    Message::Stdin(data) | Message::Stdout(data) | Message::Stderr(data) => {
                        assert_eq!(data, payload);
                    }
                    other => panic!("unexpected message: {other:?}"),
                }
            }
        }
    }

    #[test]
    fn statuses_and_failures_round_trip() {
        for value in [i32::MIN, -1, 0, 255, i32::MAX] {
            assert!(matches!(
                PostcardCodec::new().decode(&mut frame(Message::Code(value))).unwrap(),
                Some(Message::Code(actual)) if actual == value
            ));
            assert!(matches!(
                PostcardCodec::new().decode(&mut frame(Message::Signaled(value))).unwrap(),
                Some(Message::Signaled(actual)) if actual == value
            ));
        }
        assert!(matches!(
            PostcardCodec::new().decode(&mut frame(Message::Failed("ошибка\n".into()))).unwrap(),
            Some(Message::Failed(message)) if message == "ошибка\n"
        ));
    }

    #[test]
    fn frames_use_a_big_endian_length_and_keep_message_order() {
        let mut bytes = frame(Message::Code(0));
        assert_eq!(bytes.as_ref(), &[0, 0, 0, 2, 4, 0]);
        PostcardCodec::new()
            .encode(Message::Code(7), &mut bytes)
            .unwrap();
        let mut codec = PostcardCodec::new();
        assert!(matches!(
            codec.decode(&mut bytes).unwrap(),
            Some(Message::Code(0))
        ));
        assert!(matches!(
            codec.decode(&mut bytes).unwrap(),
            Some(Message::Code(7))
        ));
        assert!(codec.decode(&mut bytes).unwrap().is_none());
    }

    #[test]
    fn incomplete_frames_wait_for_the_remaining_bytes_at_every_split() {
        let encoded = frame(Message::Stdout(b"chunked payload".to_vec()));
        for split in 0..encoded.len() {
            let mut codec = PostcardCodec::new();
            let mut bytes = BytesMut::from(&encoded[..split]);
            assert!(codec.decode(&mut bytes).unwrap().is_none(), "split {split}");
            bytes.extend_from_slice(&encoded[split..]);
            assert!(matches!(
                codec.decode(&mut bytes).unwrap(),
                Some(Message::Stdout(data)) if data == b"chunked payload"
            ));
            assert!(bytes.is_empty());
        }
    }

    #[test]
    fn malformed_payloads_and_oversized_frames_are_rejected() {
        for bytes in [
            &[0, 0, 0, 0][..],
            &[0, 0, 0, 1, 255][..],
            &[0, 0, 0, 1, 4][..],
            &[255, 255, 255, 255][..],
        ] {
            let error = PostcardCodec::new()
                .decode(&mut BytesMut::from(bytes))
                .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn eof_distinguishes_a_clean_stream_from_a_truncated_frame() {
        let mut codec = PostcardCodec::new();
        assert!(codec.decode_eof(&mut BytesMut::new()).unwrap().is_none());
        for bytes in [&[0, 0][..], &[0, 0, 0, 2, 4][..]] {
            assert!(
                PostcardCodec::new()
                    .decode_eof(&mut BytesMut::from(bytes))
                    .is_err()
            );
        }
    }

    #[test]
    fn encoding_rejects_payloads_above_the_frame_limit() {
        let mut bytes = BytesMut::new();
        let result =
            PostcardCodec::new().encode(Message::Stdout(vec![0; 8 * 1024 * 1024]), &mut bytes);
        assert!(result.is_err());
        assert!(bytes.is_empty());
    }
}
