//! Checked little-endian lengths and ordered hash lists for durable metadata.
use super::commit_hash::CommitHash;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

pub(crate) open spec fn hash_bytes(hashes: Seq<CommitHash>) -> Seq<u8> {
    Seq::new(32 * hashes.len(), |i: int| hashes[i / 32].0@[i % 32])
}
pub(crate) open spec fn encoded_hashes(hashes: Seq<CommitHash>) -> Seq<u8> {
    vstd::bytes::spec_u64_to_le_bytes(hashes.len() as u64) + hash_bytes(hashes)
}

pub(crate) fn encode_u64(out: &mut Vec<u8>, n: u64)
    ensures final(out)@ == old(out)@ + vstd::bytes::spec_u64_to_le_bytes(n),
{
    proof { vstd::bytes::spec_u64_to_le_bytes_to_open(n); }
    out.extend_from_slice(&[
        (n & 0xff) as u8, ((n >> 8) & 0xff) as u8, ((n >> 16) & 0xff) as u8, ((n >> 24) & 0xff) as u8,
        ((n >> 32) & 0xff) as u8, ((n >> 40) & 0xff) as u8, ((n >> 48) & 0xff) as u8, ((n >> 56) & 0xff) as u8,
    ]);
}

pub(crate) fn encode_hashes(out: &mut Vec<u8>, hashes: &[CommitHash])
    ensures final(out)@ == old(out)@ + encoded_hashes(hashes@),
{
    let ghost prefix = out@;
    encode_u64(out, hashes.len() as u64);
    let ghost header = out@;
    let mut i = 0usize;
    while i < hashes.len()
        invariant i <= hashes.len(), header == prefix + vstd::bytes::spec_u64_to_le_bytes(hashes.len() as u64),
            out@ == header + hash_bytes(hashes@.take(i as int)),
        decreases hashes.len() - i,
    {
        proof {
            let before = hash_bytes(hashes@.take(i as int));
            assert(hash_bytes(hashes@.take(i + 1)) =~= before + hashes@[i as int].0@) by {
                assert forall|j: int| 0 <= j < 32*(i+1) implies
                    #[trigger] hash_bytes(hashes@.take(i+1))[j] == (before + hashes@[i as int].0@)[j] by {
                    if j < 32*i {} else {
                        assert(j / 32 == i && j % 32 == j - 32*i) by(nonlinear_arith)
                            requires 32*i <= j < 32*(i+1), i >= 0;
                    }
                }
            }
        }
        out.extend_from_slice(&hashes[i].0);
        i += 1;
    }
    assert(hashes@.take(i as int) == hashes@);
}

pub(crate) proof fn u64_roundtrip(n: u64)
    ensures vstd::bytes::spec_u64_to_le_bytes(n).len() == 8,
        super::reader::prefix_count(vstd::bytes::spec_u64_to_le_bytes(n)) == n,
{
    vstd::bytes::spec_u64_to_le_bytes_to_open(n);
    assert((((n & 0xff) as u8) as u64)
        | ((((n >> 8) & 0xff) as u8) as u64) << 8
        | ((((n >> 16) & 0xff) as u8) as u64) << 16
        | ((((n >> 24) & 0xff) as u8) as u64) << 24
        | ((((n >> 32) & 0xff) as u8) as u64) << 32
        | ((((n >> 40) & 0xff) as u8) as u64) << 40
        | ((((n >> 48) & 0xff) as u8) as u64) << 48
        | ((((n >> 56) & 0xff) as u8) as u64) << 56 == n) by(bit_vector);
}

/// Every eight-byte word has exactly its original little-endian representation.
pub(crate) proof fn u64_bytes_roundtrip(bytes: Seq<u8>)
    requires bytes.len() == 8,
    ensures vstd::bytes::spec_u64_to_le_bytes(super::reader::prefix_count(bytes)) == bytes,
{
    let n = super::reader::prefix_count(bytes);
    vstd::bytes::spec_u64_to_le_bytes_to_open(n);
    let b0 = bytes[0];
    let b1 = bytes[1];
    let b2 = bytes[2];
    let b3 = bytes[3];
    let b4 = bytes[4];
    let b5 = bytes[5];
    let b6 = bytes[6];
    let b7 = bytes[7];
    assert((((((b0 as u64) | (b1 as u64) << 8 | (b2 as u64) << 16 | (b3 as u64) << 24 | (b4 as u64) << 32 | (b5 as u64) << 40 | (b6 as u64) << 48 | (b7 as u64) << 56)) & 0xff) as u8) == b0) by(bit_vector);
    assert((((((b0 as u64) | (b1 as u64) << 8 | (b2 as u64) << 16 | (b3 as u64) << 24 | (b4 as u64) << 32 | (b5 as u64) << 40 | (b6 as u64) << 48 | (b7 as u64) << 56) >> 8) & 0xff) as u8) == b1) by(bit_vector);
    assert((((((b0 as u64) | (b1 as u64) << 8 | (b2 as u64) << 16 | (b3 as u64) << 24 | (b4 as u64) << 32 | (b5 as u64) << 40 | (b6 as u64) << 48 | (b7 as u64) << 56) >> 16) & 0xff) as u8) == b2) by(bit_vector);
    assert((((((b0 as u64) | (b1 as u64) << 8 | (b2 as u64) << 16 | (b3 as u64) << 24 | (b4 as u64) << 32 | (b5 as u64) << 40 | (b6 as u64) << 48 | (b7 as u64) << 56) >> 24) & 0xff) as u8) == b3) by(bit_vector);
    assert((((((b0 as u64) | (b1 as u64) << 8 | (b2 as u64) << 16 | (b3 as u64) << 24 | (b4 as u64) << 32 | (b5 as u64) << 40 | (b6 as u64) << 48 | (b7 as u64) << 56) >> 32) & 0xff) as u8) == b4) by(bit_vector);
    assert((((((b0 as u64) | (b1 as u64) << 8 | (b2 as u64) << 16 | (b3 as u64) << 24 | (b4 as u64) << 32 | (b5 as u64) << 40 | (b6 as u64) << 48 | (b7 as u64) << 56) >> 40) & 0xff) as u8) == b5) by(bit_vector);
    assert((((((b0 as u64) | (b1 as u64) << 8 | (b2 as u64) << 16 | (b3 as u64) << 24 | (b4 as u64) << 32 | (b5 as u64) << 40 | (b6 as u64) << 48 | (b7 as u64) << 56) >> 48) & 0xff) as u8) == b6) by(bit_vector);
    assert((((((b0 as u64) | (b1 as u64) << 8 | (b2 as u64) << 16 | (b3 as u64) << 24 | (b4 as u64) << 32 | (b5 as u64) << 40 | (b6 as u64) << 48 | (b7 as u64) << 56) >> 56) & 0xff) as u8) == b7) by(bit_vector);
    assert(vstd::bytes::spec_u64_to_le_bytes(n)[0] == b0);
    assert(vstd::bytes::spec_u64_to_le_bytes(n)[1] == b1);
    assert(vstd::bytes::spec_u64_to_le_bytes(n)[2] == b2);
    assert(vstd::bytes::spec_u64_to_le_bytes(n)[3] == b3);
    assert(vstd::bytes::spec_u64_to_le_bytes(n)[4] == b4);
    assert(vstd::bytes::spec_u64_to_le_bytes(n)[5] == b5);
    assert(vstd::bytes::spec_u64_to_le_bytes(n)[6] == b6);
    assert(vstd::bytes::spec_u64_to_le_bytes(n)[7] == b7);
    assert(vstd::bytes::spec_u64_to_le_bytes(n) =~= bytes);
}

/// The count and every 32-byte member retain their exact positions, including duplicates.
pub(crate) proof fn hash_layout(hashes: Seq<CommitHash>)
    requires hashes.len() <= u64::MAX,
    ensures encoded_hashes(hashes).len() == 8 + 32*hashes.len(),
        super::reader::prefix_count(encoded_hashes(hashes)) == hashes.len(),
        forall|i: int| 0 <= i < hashes.len() ==> encoded_hashes(hashes).subrange(8 + 32*i, 8 + 32*(i+1)) == hashes[i].0@,
{
    u64_roundtrip(hashes.len() as u64);
    assert(encoded_hashes(hashes).take(8) == vstd::bytes::spec_u64_to_le_bytes(hashes.len() as u64));
    assert forall|i: int| 0 <= i < hashes.len() implies
        #[trigger] encoded_hashes(hashes).subrange(8+32*i,8+32*(i+1)) == hashes[i].0@ by {
        assert forall|j: int| 0 <= j < 32 implies
            #[trigger] encoded_hashes(hashes).subrange(8+32*i,8+32*(i+1))[j] == hashes[i].0@[j] by {
            assert((32*i+j)/32 == i && (32*i+j)%32 == j) by(nonlinear_arith)
                requires 0 <= j < 32, i >= 0;
        }
        assert(encoded_hashes(hashes).subrange(8+32*i,8+32*(i+1)) =~= hashes[i].0@);
    }
}
} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::Reader;

    #[test]
    fn integer_codec_matches_native_little_endian_and_preserves_suffixes() {
        let mut values = vec![0, 1, 255, 256, u32::MAX as u64, u64::MAX];
        for bit in 0..64 {
            let edge = 1u64 << bit;
            values.extend([edge - 1, edge, edge.saturating_add(1)]);
        }
        let mut n = 71u64;
        for _ in 0..10_000 {
            n = n
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            values.push(n);
        }
        for n in values {
            let mut bytes = vec![99];
            encode_u64(&mut bytes, n);
            assert_eq!(&bytes[1..], &n.to_le_bytes());
            bytes.extend([17, 18]);
            let mut reader = Reader::new(&bytes[1..]);
            assert_eq!(reader.u64_le().unwrap(), n);
            assert_eq!(reader.rest(), &[17, 18]);
            for len in 0..8 {
                let mut short = Reader::new(&bytes[1..1 + len]);
                assert!(short.u64_le().is_err());
                assert_eq!(short.rest(), &bytes[1..1 + len]);
            }
        }
    }

    #[test]
    fn hash_codec_preserves_duplicates_and_checks_untrusted_lengths() {
        for hashes in [
            vec![],
            vec![CommitHash([3; 32])],
            vec![
                CommitHash([7; 32]),
                CommitHash([3; 32]),
                CommitHash([7; 32]),
            ],
        ] {
            let mut bytes = Vec::new();
            encode_hashes(&mut bytes, &hashes);
            let mut expected = (hashes.len() as u64).to_le_bytes().to_vec();
            for hash in &hashes {
                expected.extend(hash.0);
            }
            assert_eq!(bytes, expected);
            for len in 0..bytes.len() {
                let mut truncated = Reader::new(&bytes[..len]);
                assert!(truncated.hashes().is_err());
                assert_eq!(
                    truncated.rest(),
                    if len < 8 {
                        &bytes[..len]
                    } else {
                        &bytes[8..len]
                    }
                );
            }
            bytes.push(99);
            let mut reader = Reader::new(&bytes);
            assert_eq!(reader.hashes().unwrap(), hashes);
            assert_eq!(reader.rest(), &[99]);
        }
        let bytes = u64::MAX.to_le_bytes();
        assert!(Reader::new(&bytes).hashes().is_err());
    }
}
