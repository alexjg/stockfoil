//! Application payloads only. Stockfoil owns framing, hashes and signatures.
use crate::Assignment;
use std::io::{Error, ErrorKind};
use stockfoil::{DecodeLimits, FragmentShape, PayloadCodec};

#[derive(Clone, Copy)]
pub(crate) struct RegisterCodec;
fn invalid() -> Error {
    Error::new(
        ErrorKind::InvalidData,
        "invalid or oversized register payload",
    )
}
fn size(n: usize) -> usize {
    let mut n = n;
    let mut bytes = 1;
    while n >= 128 {
        bytes += 1;
        n >>= 7;
    }
    bytes
}
struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn raw(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self.offset.checked_add(n).ok_or_else(invalid)?;
        let result = self.bytes.get(self.offset..end).ok_or_else(invalid)?;
        self.offset = end;
        Ok(result)
    }
    fn count(&mut self, max: usize) -> Result<usize, Error> {
        let mut n = 0u32;
        for i in 0..5 {
            let b = self.raw(1)?[0];
            if i == 4 && b > 15 {
                return Err(invalid());
            }
            n |= u32::from(b & 127) << (7 * i);
            if b & 128 == 0 {
                if (i > 0 && b == 0) || n as usize > max {
                    return Err(invalid());
                }
                return Ok(n as usize);
            }
        }
        Err(invalid())
    }
    fn string(&mut self) -> Result<&'a str, Error> {
        let n = self.count(self.bytes.len() - self.offset)?;
        std::str::from_utf8(self.raw(n)?).map_err(|_| invalid())
    }
    fn u32(&mut self) -> Result<usize, Error> {
        Ok(u32::from_le_bytes(self.raw(4)?.try_into().unwrap()) as usize)
    }
    fn finish(&self) -> Result<(), Error> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}
impl PayloadCodec for RegisterCodec {
    type Payload = Assignment;
    type Error = Error;
    fn format_id(&self) -> &str {
        "stockfoil/mv-register/v1"
    }
    fn encode_commit(&self, payload: &Assignment) -> Result<Vec<u8>, Error> {
        bcs::to_bytes(payload).map_err(|_| invalid())
    }
    fn decode_commit(&self, bytes: &[u8], limits: &DecodeLimits) -> Result<Assignment, Error> {
        if bytes.len() > limits.max_item_bytes || bytes.len() > limits.max_payload_bytes {
            return Err(invalid());
        }
        let mut r = Reader { bytes, offset: 0 };
        let key = r.string()?;
        let value = r.string()?;
        r.finish()?;
        Ok(Assignment {
            key: key.into(),
            value: value.into(),
        })
    }
    fn encode_fragment(
        &self,
        shape: &FragmentShape,
        payloads: &[&Assignment],
    ) -> Result<Vec<u8>, Error> {
        if shape.members.len() != payloads.len() {
            return Err(invalid());
        }
        let mut keys: Vec<&str> = Vec::new();
        let mut indices = std::collections::HashMap::new();
        let mut runs: Vec<(u32, u32)> = Vec::new();
        let mut values = Vec::new();
        for p in payloads {
            let index = *indices.entry(p.key.as_str()).or_insert_with(|| {
                let i = keys.len() as u32;
                keys.push(&p.key);
                i
            });
            if let Some((_, count)) = runs.last_mut().filter(|(last, _)| *last == index) {
                *count = count.checked_add(1).ok_or_else(invalid)?;
            } else {
                runs.push((index, 1));
            }
            values.push(p.value.as_str());
        }
        bcs::to_bytes(&(keys, runs, values)).map_err(|_| invalid())
    }
    fn decode_fragment(
        &self,
        shape: &FragmentShape,
        bytes: &[u8],
        limits: &DecodeLimits,
    ) -> Result<Vec<Assignment>, Error> {
        let n = shape.members.len();
        // Every canonical assignment has at least two length bytes.
        if bytes.len() > limits.max_item_bytes
            || n > limits.max_members
            || n > limits.max_payload_bytes / 2
        {
            return Err(invalid());
        }
        let mut r = Reader { bytes, offset: 0 };
        let count = r.count(n.min(bytes.len()))?;
        let mut keys = Vec::new();
        let mut unique = std::collections::HashSet::new();
        for _ in 0..count {
            let key = r.string()?;
            if !unique.insert(key) {
                return Err(invalid());
            }
            keys.push(key);
        }
        let count = r.count(n.min((bytes.len() - r.offset) / 8))?;
        let mut runs = Vec::new();
        let mut total = 0usize;
        let mut budget = 0usize;
        let mut seen = 0;
        for _ in 0..count {
            let index = r.u32()?;
            let len = r.u32()?;
            let key = keys.get(index).ok_or_else(invalid)?;
            if len == 0 || index > seen || runs.last().is_some_and(|&(i, _)| i == index) {
                return Err(invalid());
            }
            if index == seen {
                seen += 1;
            }
            total = total.checked_add(len).ok_or_else(invalid)?;
            budget = budget
                .checked_add(
                    (key.len() + size(key.len()))
                        .checked_mul(len)
                        .ok_or_else(invalid)?,
                )
                .ok_or_else(invalid)?;
            if total > n || budget > limits.max_payload_bytes {
                return Err(invalid());
            }
            runs.push((index, len));
        }
        if total != n || seen != keys.len() || r.count(n)? != n {
            return Err(invalid());
        }
        // Validate all strings and the expanded canonical byte budget before cloning keys.
        let mut values = Vec::new();
        for _ in 0..n {
            let value = r.string()?;
            budget = budget
                .checked_add(value.len() + size(value.len()))
                .ok_or_else(invalid)?;
            if budget > limits.max_payload_bytes {
                return Err(invalid());
            }
            values.push(value);
        }
        r.finish()?;
        let mut values = values.into_iter();
        let mut result = Vec::new();
        for (index, len) in runs {
            for _ in 0..len {
                result.push(Assignment {
                    key: keys[index].into(),
                    value: values.next().unwrap().into(),
                });
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stockfoil::{Author, MemberShape};
    fn shape(n: usize) -> FragmentShape {
        FragmentShape {
            members: (0..n)
                .map(|_| MemberShape {
                    author: Author::try_from([0; 32].as_slice()).unwrap(),
                    parents: vec![],
                })
                .collect(),
        }
    }
    #[test]
    fn rust_js_golden_encodings_cross_default_budget_boundary() {
        // Shared with the pure JS payload tests; prefixes are followed by n 'x's.
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Vector {
            value_bytes: usize,
            commit_prefix: String,
            fragment_prefix: String,
        }
        fn golden(prefix: &str, n: usize) -> Vec<u8> {
            let mut bytes = hex::decode(prefix).unwrap();
            bytes.resize(bytes.len() + n, b'x');
            bytes
        }
        let vectors: Vec<Vector> = serde_json::from_str(include_str!(
            "../../mv-register-js/test/fixtures/codec-boundaries.json"
        ))
        .unwrap();
        for vector in vectors {
            let payload = Assignment {
                key: "k".into(),
                value: "x".repeat(vector.value_bytes),
            };
            let commit = RegisterCodec.encode_commit(&payload).unwrap();
            let fragment = RegisterCodec
                .encode_fragment(&shape(1), &[&payload])
                .unwrap();
            assert_eq!(commit, golden(&vector.commit_prefix, vector.value_bytes));
            assert_eq!(
                fragment,
                golden(&vector.fragment_prefix, vector.value_bytes)
            );
            let limits = DecodeLimits {
                max_item_bytes: fragment.len(),
                max_payload_bytes: commit.len(),
                max_members: 1,
                ..DecodeLimits::default()
            };
            let decoded = RegisterCodec.decode_commit(&commit, &limits).unwrap();
            assert_eq!(
                (&decoded.key, &decoded.value),
                (&payload.key, &payload.value)
            );
            let decoded = RegisterCodec
                .decode_fragment(&shape(1), &fragment, &limits)
                .unwrap();
            assert_eq!(
                (&decoded[0].key, &decoded[0].value),
                (&payload.key, &payload.value)
            );
            for tight in [
                DecodeLimits {
                    max_item_bytes: commit.len() - 1,
                    ..limits
                },
                DecodeLimits {
                    max_payload_bytes: commit.len() - 1,
                    ..limits
                },
            ] {
                assert!(RegisterCodec.decode_commit(&commit, &tight).is_err());
            }
            for tight in [
                DecodeLimits {
                    max_item_bytes: fragment.len() - 1,
                    ..limits
                },
                DecodeLimits {
                    max_payload_bytes: commit.len() - 1,
                    ..limits
                },
            ] {
                assert!(
                    RegisterCodec
                        .decode_fragment(&shape(1), &fragment, &tight)
                        .is_err()
                );
            }
            assert_eq!(
                RegisterCodec
                    .decode_commit(&commit, &DecodeLimits::default())
                    .is_ok(),
                commit.len() <= DecodeLimits::default().max_payload_bytes,
            );
            assert!(
                RegisterCodec
                    .decode_fragment(&shape(1), &fragment, &DecodeLimits::default())
                    .is_err()
            );
        }
    }

    #[test]
    fn shared_bcs_fragment_grammar_and_budgets() {
        let a = Assignment {
            key: "k".into(),
            value: "a".into(),
        };
        let b = Assignment {
            key: "k".into(),
            value: "b".into(),
        };
        let bytes = RegisterCodec.encode_fragment(&shape(2), &[&a, &b]).unwrap();
        assert_eq!(hex::encode(&bytes), "01016b0100000000020000000201610162");
        let limits = DecodeLimits::default();
        assert_eq!(
            RegisterCodec
                .decode_fragment(&shape(2), &bytes, &limits)
                .unwrap()[1]
                .value,
            "b"
        );
        for len in 0..bytes.len() {
            assert!(
                RegisterCodec
                    .decode_fragment(&shape(2), &bytes[..len], &limits)
                    .is_err()
            );
        }
        let tight = DecodeLimits {
            max_payload_bytes: 7,
            ..limits
        };
        assert!(
            RegisterCodec
                .decode_fragment(&shape(2), &bytes, &tight)
                .is_err()
        );
        for (offset, value) in [(8, u32::MAX), (8, 0), (4, 1)] {
            let mut malformed = bytes.clone();
            malformed[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(
                RegisterCodec
                    .decode_fragment(&shape(2), &malformed, &DecodeLimits::default())
                    .is_err()
            );
        }
        let mut trailing = bytes;
        trailing.push(0);
        assert!(
            RegisterCodec
                .decode_fragment(&shape(2), &trailing, &DecodeLimits::default())
                .is_err()
        );
    }

    #[test]
    fn dictionary_first_occurrence_order_and_canonical_assignment() {
        let a = Assignment {
            key: "z".into(),
            value: "1".into(),
        };
        let b = Assignment {
            key: "a".into(),
            value: "2".into(),
        };
        let c = Assignment {
            key: "z".into(),
            value: "3".into(),
        };
        let bytes = RegisterCodec
            .encode_fragment(&shape(3), &[&a, &b, &c])
            .unwrap();
        assert_eq!(
            hex::encode(&bytes),
            "02017a01610300000000010000000100000001000000000000000100000003013101320133"
        );
        let decoded = RegisterCodec
            .decode_fragment(&shape(3), &bytes, &DecodeLimits::default())
            .unwrap();
        assert_eq!(
            decoded.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
            vec!["z", "a", "z"]
        );
        let assignment = Assignment {
            key: "title".into(),
            value: "Hello".into(),
        };
        assert_eq!(
            hex::encode(RegisterCodec.encode_commit(&assignment).unwrap()),
            "057469746c650548656c6c6f"
        );
        for malformed in [vec![0x80, 0, 0], vec![1, 0xff, 0], vec![0, 0, 0]] {
            assert!(
                RegisterCodec
                    .decode_commit(&malformed, &DecodeLimits::default())
                    .is_err()
            );
        }
    }
}
