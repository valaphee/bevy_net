use std::io::{Read, Write};

use aes::{
    cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit},
    Aes128,
};
use byteorder::{ReadBytesExt, WriteBytesExt};
use bytes::{Buf, BufMut, BytesMut};
use cfb8::{Decryptor, Encryptor};
use flate2::read::{ZlibDecoder, ZlibEncoder};
pub use flate2::Compression;
use tokio_util::codec::{Decoder, Encoder};

use crate::{Error, Result};

#[derive(Default)]
pub struct Codec {
    encryptor: Option<Encryptor<Aes128>>,
    decryptor: Option<Decryptor<Aes128>>,
    decrypted_bytes: usize,

    compression: Compression,
    compression_threshold: Option<u16>,
}

impl Codec {
    pub fn enable_encryption(&mut self, key: &[u8]) {
        self.encryptor = Some(Encryptor::new_from_slices(key, key).unwrap());
        self.decryptor = Some(Decryptor::new_from_slices(key, key).unwrap());
        self.decrypted_bytes = 0;
    }

    pub fn enable_compression(&mut self, compression: Compression, compression_threshold: u16) {
        assert!(compression_threshold <= 16384);
        self.compression = compression;
        self.compression_threshold = Some(compression_threshold);
    }
}

impl Encoder<&[u8]> for Codec {
    type Error = Error;

    fn encode(&mut self, item: &[u8], dst: &mut BytesMut) -> Result<()> {
        let data_length_offset = dst.len();
        dst.put_bytes(0, 3);
        let data_offset = dst.len();
        dst.put_slice(item);
        let mut data_length = dst.len() - data_offset;

        if let Some(compression_threshold) = self.compression_threshold {
            if data_length > compression_threshold as usize {
                let mut compressed_data = Vec::new();
                ZlibEncoder::new(&dst[data_offset..], self.compression)
                    .read_to_end(&mut compressed_data)
                    .unwrap();

                dst.truncate(data_length_offset);
                let mut writer = dst.writer();
                varint32_encode((varint32_length(data_length as i32) + compressed_data.len()) as i32, &mut writer);
                varint32_encode(data_length as i32, &mut writer);
                dst.extend_from_slice(&compressed_data);
            } else {
                data_length += 1;

                // This will limit the maximum compression threshold to 16384 (2 VarInt bytes)
                // as the third VarInt byte has to be kept zero to indicate no
                // compression.
                let data_length_data = &mut dst[data_length_offset..data_offset];
                data_length_data[0] = (data_length & 0x7F) as u8 | 0x80;
                data_length_data[1] = (data_length >> 7 & 0x7F) as u8;
            }
        } else {
            let data_length_data = &mut dst[data_length_offset..data_offset];
            data_length_data[0] = (data_length & 0x7F) as u8 | 0x80;
            data_length_data[1] = (data_length >> 7 & 0x7F) as u8 | 0x80;
            data_length_data[2] = (data_length >> 14 & 0x7F) as u8;
        }

        // Encrypt written bytes
        if let Some(encryptor) = &mut self.encryptor {
            encryptor
                .encrypt_blocks_mut(unsafe { std::mem::transmute(&mut dst[data_length_offset..]) });
        }

        Ok(())
    }
}

impl Decoder for Codec {
    type Item = Vec<u8>;
    type Error = Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>> {
        // Decrypt all not yet decrypted bytes
        if let Some(decryptor) = &mut self.decryptor {
            decryptor.decrypt_blocks_mut(unsafe {
                std::mem::transmute(&mut src[self.decrypted_bytes..])
            });
            self.decrypted_bytes = src.len();
        }

        let mut data = &src[..];
        match varint21_decode(&mut data) {
            Ok(data_length) => {
                if data.len() >= data_length as usize {
                    data = &data[..data_length as usize];

                    let data = if self.compression_threshold.is_some() {
                        let decompressed_data_length = varint32_decode(&mut data)?;
                        if decompressed_data_length != 0 {
                            let mut decompressed_data =
                                Vec::with_capacity(decompressed_data_length as usize);
                            ZlibDecoder::new(data)
                                .read_to_end(&mut decompressed_data)
                                .unwrap();
                            decompressed_data
                        } else {
                            data.to_vec()
                        }
                    } else {
                        data.to_vec()
                    };

                    // Advance, and correct decrypted bytes
                    src.advance(varint32_length(data_length) + data_length as usize);
                    if self.decryptor.is_some() {
                        self.decrypted_bytes = src.len()
                    }

                    Ok(Some(data))
                } else {
                    Ok(None)
                }
            }
            Err(error) => {
                if data.len() >= 3 {
                    Err(error)
                } else {
                    Ok(None)
                }
            }
        }
    }
}

fn varint32_length(value: i32) -> usize {
    match value {
        0 => 1,
        n => (31 - n.leading_zeros() as usize) / 7 + 1,
    }
}

fn varint32_encode(mut value: i32, output: &mut impl Write) -> Result<()> {
    loop {
        if value & !0b01111111 == 0 {
            output.write_u8(value as u8)?;
            return Ok(());
        }
        output.write_u8(value as u8 & 0b01111111 | 0b10000000)?;
        value >>= 7;
    }
}

fn varint32_decode(input: &mut &[u8]) -> Result<i32> {
    let mut value = 0;
    let mut shift = 0;
    while shift <= 35 {
        let head = input.read_u8()?;
        value |= (head as i32 & 0b01111111) << shift;
        if head & 0b10000000 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(Error::VarIntTooWide(35))
}

fn varint21_decode(input: &mut &[u8]) -> Result<i32> {
    let mut value = 0;
    let mut shift = 0;
    while shift <= 21 {
        let head = input.read_u8()?;
        value |= (head as i32 & 0b01111111) << shift;
        if head & 0b10000000 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(Error::VarIntTooWide(21))
}
