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
