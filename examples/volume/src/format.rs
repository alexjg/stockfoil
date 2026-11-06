use crate::{Error, Region, Result, Spool};
use stockfoil::DecodeStatus;

pub const BLOCK_BYTES: usize = 64 * 1024;
pub const FORMAT: &str = "stockfoil/volume/v1";

/// Body framing: u64 LE canonical length, then canonical bytes, per member.
/// No compression/deltas; only eight bytes of decoder state plus counters.
pub struct BodyDecoder {
    members: usize,
    length: [u8; 8],
    have: usize,
    remaining: Option<u64>,
}
impl BodyDecoder {
    pub fn new(members: usize) -> Self {
        Self {
            members,
            length: [0; 8],
            have: 0,
            remaining: None,
        }
    }
    pub fn step(
        &mut self,
        input: &[u8],
        eof: bool,
        out: &mut [u8],
    ) -> Result<(usize, usize, DecodeStatus)> {
        if self.members == 0 {
            if !input.is_empty() {
                return Err(Error::Invalid("trailing bundle bytes"));
            }
            return Ok((0, 0, DecodeStatus::Finished));
        }
        let mut consumed = 0;
        if self.remaining.is_none() {
            let n = (8 - self.have).min(input.len());
            self.length[self.have..self.have + n].copy_from_slice(&input[..n]);
            self.have += n;
            consumed += n;
            if self.have == 8 {
                self.remaining = Some(u64::from_le_bytes(self.length));
                self.have = 0;
            } else if eof {
                return Err(Error::Invalid("short member length"));
            } else {
                return Ok((consumed, 0, DecodeStatus::NeedInput));
            }
        }
        let remaining = self.remaining.as_mut().unwrap();
        let n = (*remaining)
            .min((input.len() - consumed) as u64)
            .min(out.len() as u64) as usize;
        out[..n].copy_from_slice(&input[consumed..consumed + n]);
        consumed += n;
        *remaining -= n as u64;
        let status = if *remaining == 0 {
            self.remaining = None;
            self.members -= 1;
            DecodeStatus::EndMember
        } else if consumed == input.len() {
            if eof {
                return Err(Error::Invalid("short member body"));
            }
            DecodeStatus::NeedInput
        } else {
            DecodeStatus::Working
        };
        Ok((consumed, n, status))
    }
}

/// Canonical payload: count, increasing u64 block indices, full block contents.
/// Parsing is two bounded scans of the header, never an array of all blocks.
#[derive(Clone, Copy)]
pub(crate) struct BlockWrite {
    pub region: Region,
    pub count: u64,
}
impl BlockWrite {
    pub fn parse(spool: &Spool, region: Region, volume_blocks: u64) -> Result<Self> {
        if region.len < 8 {
            return Err(Error::Invalid("short block-map header"));
        }
        let mut word = [0; 8];
        spool.read(region, 0, &mut word)?;
        let count = u64::from_le_bytes(word);
        let expected = count
            .checked_mul(8 + BLOCK_BYTES as u64)
            .and_then(|n| n.checked_add(8));
        if expected != Some(region.len) || count > volume_blocks {
            return Err(Error::Invalid("invalid block-map length/count"));
        }
        let write = Self { region, count };
        let mut previous = None;
        for nth in 0..count {
            let block = write.block(spool, nth)?;
            if block >= volume_blocks || previous.is_some_and(|p| p >= block) {
                return Err(Error::Invalid(
                    "block indices out of range or not increasing",
                ));
            }
            previous = Some(block);
        }
        Ok(write)
    }
    pub fn block(self, spool: &Spool, nth: u64) -> Result<u64> {
        let mut word = [0; 8];
        spool.read(self.region, 8 + nth * 8, &mut word)?;
        Ok(u64::from_le_bytes(word))
    }
    pub fn read(self, spool: &Spool, nth: u64, out: &mut [u8; BLOCK_BYTES]) -> Result<()> {
        if nth >= self.count {
            return Err(Error::Invalid("block ordinal beyond payload"));
        }
        spool.read(
            self.region,
            8 + self.count * 8 + nth * BLOCK_BYTES as u64,
            out,
        )
    }
}
