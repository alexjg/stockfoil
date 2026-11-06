//! Key-deduplicating bundle codec for register [`Assignment`]s (`stockfoil/mv-register/v2`).
//!
//! Canonical member payload:
//! ```text
//! uleb(key.len) | key | uleb(value.len) | value
//! ```
//!
//! Encoded bundle body (one entry per member in `meta`):
//! - `0x00 | uleb(key.len) | key | uleb(value.len) | value` (literal key; added to
//!   the bundle dictionary while capacity permits)
//! - `0x01 | uleb(dict_index) | uleb(value.len) | value` (reference to an earlier key)
use crate::Assignment;
use std::collections::HashMap;
use stockfoil::{BundleMeta, Bytes, Codec, FormatId};

pub const FORMAT: &str = "stockfoil/mv-register/v2";
pub const MAX_KEY_BYTES: usize = 1024;
const MAX_DICT_KEYS: usize = 4096;
const MAX_DICT_BYTES: usize = 256 * 1024;

const LITERAL: u8 = 0;
const REFERENCE: u8 = 1;

pub(crate) fn put_uleb(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn take_uleb(rest: &mut &[u8]) -> Result<u64, &'static str> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let (&byte, tail) = rest.split_first().ok_or("truncated uleb")?;
        *rest = tail;
        let low = u64::from(byte & 0x7f);
        let term = low.checked_shl(shift).ok_or("uleb overflow")?;
        if (term >> shift) != low {
            return Err("uleb overflow");
        }
        value |= term;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
}

fn take_str<'a>(rest: &mut &'a [u8], len: usize) -> Result<&'a str, &'static str> {
    if rest.len() < len {
        return Err("truncated slice");
    }
    let (head, tail) = rest.split_at(len);
    *rest = tail;
    std::str::from_utf8(head).map_err(|_| "invalid UTF-8")
}

#[derive(Clone, Debug)]
pub struct RegisterCodec {
    format: FormatId,
}

impl Default for RegisterCodec {
    fn default() -> Self {
        Self {
            format: FormatId::new(FORMAT).unwrap(),
        }
    }
}

impl RegisterCodec {
    pub fn encode_body(&self, payloads: &[&Assignment]) -> Result<Vec<u8>, &'static str> {
        let mut out = Vec::new();
        let mut dictionary: HashMap<&str, usize> = HashMap::new();
        let mut dict_bytes = 0usize;
        for &assignment in payloads {
            let key = assignment.key.as_str();
            if key.len() > MAX_KEY_BYTES {
                return Err("key exceeds MAX_KEY_BYTES");
            }
            if let Some(&idx) = dictionary.get(key) {
                out.push(REFERENCE);
                put_uleb(&mut out, idx as u64);
            } else {
                out.push(LITERAL);
                put_uleb(&mut out, key.len() as u64);
                out.extend_from_slice(key.as_bytes());
                if dictionary.len() < MAX_DICT_KEYS && dict_bytes + key.len() <= MAX_DICT_BYTES {
                    let idx = dictionary.len();
                    dictionary.insert(key, idx);
                    dict_bytes += key.len();
                }
            }
            put_uleb(&mut out, assignment.value.len() as u64);
            out.extend_from_slice(assignment.value.as_bytes());
        }
        Ok(out)
    }

    pub fn decode_body(
        &self,
        member_count: usize,
        mut body: &[u8],
    ) -> Result<Vec<Assignment>, &'static str> {
        let mut members = Vec::with_capacity(member_count);
        let mut dictionary: Vec<&str> = Vec::new();
        let mut dict_bytes = 0usize;
        for _ in 0..member_count {
            let (&tag, tail) = body.split_first().ok_or("truncated body")?;
            body = tail;
            let key: &str = match tag {
                LITERAL => {
                    let key_len = usize::try_from(take_uleb(&mut body)?)
                        .map_err(|_| "key length overflow")?;
                    if key_len > MAX_KEY_BYTES {
                        return Err("key exceeds MAX_KEY_BYTES");
                    }
                    let key = take_str(&mut body, key_len)?;
                    if dictionary.len() < MAX_DICT_KEYS && dict_bytes + key.len() <= MAX_DICT_BYTES
                    {
                        dictionary.push(key);
                        dict_bytes += key.len();
                    }
                    key
                }
                REFERENCE => {
                    let idx = usize::try_from(take_uleb(&mut body)?)
                        .map_err(|_| "dictionary index overflow")?;
                    dictionary
                        .get(idx)
                        .copied()
                        .ok_or("unknown key reference")?
                }
                _ => return Err("unknown key tag"),
            };
            let val_len =
                usize::try_from(take_uleb(&mut body)?).map_err(|_| "value length overflow")?;
            let value = take_str(&mut body, val_len)?;
            members.push(Assignment::new(key, value));
        }
        if !body.is_empty() {
            return Err("bytes after the last member");
        }
        Ok(members)
    }
}

/// Parse one canonical assignment independently of bundle compression.
pub fn decode_assignment(canonical: &[u8]) -> Result<Assignment, &'static str> {
    let mut body = canonical;
    let key_len = usize::try_from(take_uleb(&mut body)?).map_err(|_| "key length overflow")?;
    if key_len > MAX_KEY_BYTES {
        return Err("key exceeds MAX_KEY_BYTES");
    }
    let key = take_str(&mut body, key_len)?.to_owned();
    let len = usize::try_from(take_uleb(&mut body)?).map_err(|_| "value length overflow")?;
    let value = take_str(&mut body, len)?.to_owned();
    if !body.is_empty() {
        return Err("trailing assignment bytes");
    }
    let assignment = Assignment { key, value };
    if assignment.encode().as_slice() != canonical {
        return Err("noncanonical assignment");
    }
    Ok(assignment)
}
impl Codec for RegisterCodec {
    type Error = &'static str;
    fn format(&self) -> &FormatId {
        &self.format
    }
    fn encode(&self, _meta: &BundleMeta, payloads: &[Bytes]) -> Result<Vec<u8>, Self::Error> {
        let assignments = payloads
            .iter()
            .map(|p| decode_assignment(p))
            .collect::<Result<Vec<_>, _>>()?;
        self.encode_body(&assignments.iter().collect::<Vec<_>>())
    }
    fn decode(&self, meta: &BundleMeta, body: &[u8]) -> Result<Vec<Bytes>, Self::Error> {
        Ok(self
            .decode_body(meta.len(), body)?
            .into_iter()
            .map(|a| Bytes::from(a.encode()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_repeated_across_members_are_stored_once() {
        let codec = RegisterCodec::default();
        let key = "shared-register-key".repeat(10);
        let members: Vec<Assignment> = (0..10)
            .map(|i| Assignment::new(&key, format!("v{i}")))
            .collect();
        let refs: Vec<&Assignment> = members.iter().collect();
        let body = codec.encode_body(&refs).unwrap();
        assert!(body.len() < key.len() * 2);
        assert_eq!(codec.decode_body(members.len(), &body).unwrap(), members);
    }

    #[test]
    fn rejects_malformed_and_truncated_bodies() {
        let codec = RegisterCodec::default();
        for bad in [
            vec![2, 0, 0],                                  // unknown tag
            vec![1, 0, 0],                                  // reference to empty dictionary
            vec![0, 0x81, 0x08, 0],                         // key longer than MAX_KEY_BYTES
            [vec![0, 0], vec![0xff; 10], vec![1]].concat(), // value length overflow
            vec![0, 1, b'a', 1, b'v', 0],                   // trailing byte
        ] {
            assert!(codec.decode_body(1, &bad).is_err(), "{bad:?}");
        }
    }
}
