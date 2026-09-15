use eyre::{eyre, Context};

pub(crate) struct Input<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Input<'a> {

    pub(crate) fn new(data: &'a [u8]) -> Self {
        Input { data, offset: 0 }
    }

    pub(crate) fn u64(&mut self) -> eyre::Result<u64> {
        let (value, num_bytes) = bijoux::u64::decode(&self.data[self.offset..]).wrap_err("failed to decode u64")?;
        self.offset += num_bytes;
        Ok(value)
    }

    pub(crate) fn bytes(&mut self, len: usize) -> eyre::Result<&'a [u8]> {
        if self.offset + len > self.data.len() {
            return Err(eyre!("not enough bytes to read"));
        }
        let bytes = &self.data[self.offset..self.offset + len];
        self.offset += len;
        Ok(bytes)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> eyre::Result<[u8; N]> {
        let bytes = self.bytes(N)?;
        let array = <[u8; N]>::try_from(bytes).map_err(|e| eyre!("failed to convert bytes to array: {}", e))?;
        Ok(array)
    }

    pub(crate) fn str(&mut self, len: usize) -> eyre::Result<&'a str> {
        let bytes = self.bytes(len)?;
        let s = std::str::from_utf8(bytes).wrap_err("failed to parse string")?;
        Ok(s)
    }

    pub(crate) fn byte(&mut self) -> eyre::Result<u8> {
        let byte = self.bytes(1)?.get(0).copied().ok_or_else(|| eyre!("not enough bytes to read"))?;
        Ok(byte)
    }
}

