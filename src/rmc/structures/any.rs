use std::io::{Read, Seek, Write};
use crate::endianness::{IS_BIG_ENDIAN, ReadExtensions};
use super::{string, Result, RmcSerialize};

#[derive(Debug)]
pub struct Any{
    pub name: String,
    pub data: Vec<u8>
}

impl RmcSerialize for Any{
    fn serialize(&self, writer: &mut dyn Write) -> Result<()> {
        todo!()
    }
    fn deserialize(mut reader: &mut dyn Read) -> Result<Self> {
        let name = String::deserialize(reader)?;

        // also length ?
        let len2: u32 = reader.read_struct(IS_BIG_ENDIAN)?;
        let length: u32 = reader.read_struct(IS_BIG_ENDIAN)?;

        let mut data = vec![0; length as usize];

        reader.read_exact(&mut data)?;

        Ok(
            Any{
                name,
                data
            }
        )
    }
}