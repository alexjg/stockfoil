#[derive(Debug)]
pub(crate) enum ParseError {
    InvalidInteger,
    UnexpectedEnd,
    TrailingBytes,
    InvalidUtf8,
    InvalidParentCount,
    InvalidAuthor,
}

pub(crate) struct Input<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Input<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Input { data, offset: 0 }
    }

    pub(crate) fn u64(&mut self) -> Result<u64, ParseError> {
        let (value, num_bytes) =
            bijoux::u64::decode(&self.data[self.offset..]).map_err(|_| ParseError::InvalidInteger)?;
        self.offset += num_bytes;
        Ok(value)
    }

    pub(crate) fn bytes(&mut self, len: usize) -> Result<&'a [u8], ParseError> {
        if len > self.data.len() - self.offset {
            return Err(ParseError::UnexpectedEnd);
        }
        let bytes = &self.data[self.offset..self.offset + len];
        self.offset += len;
        Ok(bytes)
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], ParseError> {
        let mut array = [0; N];
        array.copy_from_slice(self.bytes(N)?);
        Ok(array)
    }

    pub(crate) fn finish(self) -> Result<(), ParseError> {
        if self.offset == self.data.len() {
            Ok(())
        } else {
            Err(ParseError::TrailingBytes)
        }
    }

    pub(crate) fn str(&mut self, len: usize) -> Result<&'a str, ParseError> {
        let bytes = self.bytes(len)?;
        let s = core::str::from_utf8(bytes).map_err(|_| ParseError::InvalidUtf8)?;
        Ok(s)
    }

    pub(crate) fn byte(&mut self) -> Result<u8, ParseError> {
        Ok(self.bytes(1)?[0])
    }
}
