//! Checked bijou64 parent-count encoding, independent of the hashing boundary.
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
verus! {
pub open spec fn tier(n:u64) -> usize {
    if n < 248 { 0 } else if n < 504 { 1 } else if n < 66040 { 2 }
    else if n < 16843256 { 3 } else if n < 4311810552 { 4 }
    else if n < 1103823438328 { 5 } else if n < 282578800148984 { 6 }
    else if n < 72340172838076920 { 7 } else { 8 }
}
pub open spec fn offset(t:usize) -> u64 {
    match t { 0 => 0, 1 => 248, 2 => 504, 3 => 66040, 4 => 16843256,
        5 => 4311810552, 6 => 1103823438328, 7 => 282578800148984, _ => 72340172838076920 }
}
pub open spec fn encoded(n:u64) -> Seq<u8> {
    if n < 248 { seq![n as u8] }
    else { Seq::new((tier(n)+1) as nat, |i:int| if i == 0 { (247+tier(n)) as u8 }
        else { (((n-offset(tier(n))) as u64) >> ((tier(n) as int-i)*8) as u64) as u8 }) }
}
pub(crate) fn encode(n:u64) -> (out:Vec<u8>)
    ensures out@ == encoded(n), 1 <= out.len() <= 9,
{
    if n < 248 { return vec![n as u8]; }
    let t = if n < 504 { 1usize } else if n < 66040 { 2 } else if n < 16843256 { 3 }
        else if n < 4311810552 { 4 } else if n < 1103823438328 { 5 }
        else if n < 282578800148984 { 6 } else if n < 72340172838076920 { 7 } else { 8 };
    let base = match t { 1 => 248u64, 2 => 504, 3 => 66040, 4 => 16843256,
        5 => 4311810552, 6 => 1103823438328, 7 => 282578800148984, _ => 72340172838076920 };
    let payload = n-base;
    let mut out = vec![(247+t) as u8];
    let mut i = 0usize;
    while i < t
        invariant 1 <= t <= 8, t == tier(n), n >= 248, base == offset(t), payload == n-base,
            i <= t, out@ == encoded(n).subrange(0,(i+1) as int),
        decreases t-i,
    {
        out.push((payload >> ((t-i-1)*8) as u64) as u8);
        i += 1;
    }
    assert(out@ =~= encoded(n));
    out
}
} // verus!

#[cfg(test)]
mod tests {
    #[test]
    fn parent_count_encoding_matches_bijoux_at_every_tier_and_across_u64() {
        let check = |n| {
            let mut expected = Vec::new();
            bijoux::u64::encode(n, &mut expected);
            assert_eq!(super::encode(n), expected, "parent count {n}");
        };
        for n in 0..70_000 {
            check(n);
        }
        for edge in [
            248u64,
            504,
            66_040,
            16_843_256,
            4_311_810_552,
            1_103_823_438_328,
            282_578_800_148_984,
            72_340_172_838_076_920,
            u64::MAX,
        ] {
            check(edge - 1);
            check(edge);
            check(edge.saturating_add(1));
        }
        let mut n = 71u64;
        for _ in 0..10_000 {
            n = n
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            check(n);
        }
    }
}
