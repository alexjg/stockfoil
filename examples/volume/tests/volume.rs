use ed25519_dalek::SigningKey;
use std::{
    fs::File,
    io::{self, Cursor, Seek, SeekFrom},
    path::Path,
};
use stockfoil::{
    Author, CommitHash, ExportCursor, Follow, Heads, Received, WritePermission, WritePermissions,
};
use stockfoil_volume::{BLOCK_BYTES, BlockIndex, BodyDecoder, Buffers, DiskImage, Fault, Volume};
use tempfile::TempDir;

fn key(n: u8) -> SigningKey {
    SigningKey::from_bytes(&[n; 32])
}
fn author(n: u8) -> Author {
    Author::try_from(key(n).verifying_key().as_bytes().as_slice()).unwrap()
}
fn trusted(v: &mut Volume) {
    v.set_permissions(WritePermissions::from([
        (author(1), WritePermission::Unbounded),
        (author(2), WritePermission::Unbounded),
    ]))
    .unwrap();
}
fn open(path: &Path, n: u8, b: &mut Buffers) -> Volume {
    let mut v = Volume::open(path, key(n), 16, b).unwrap();
    trusted(&mut v);
    v
}
fn write(
    v: &mut Volume,
    blocks: &[u64],
    byte: u8,
    parents: Option<Vec<CommitHash>>,
    b: &mut Buffers,
) -> CommitHash {
    v.write(blocks, &mut io::repeat(byte), parents, b).unwrap()
}
fn sync(a: &Volume, b: &mut Volume, cursor: &ExportCursor, buffers: &mut Buffers) -> ExportCursor {
    let mut plan = a.ledger().export(cursor);
    for i in 0..plan.len() {
        let item = a.ledger().export_item(&plan, i).unwrap();
        let mut wire = tempfile::tempfile().unwrap();
        a.export_item(&item, &mut wire, buffers).unwrap();
        wire.seek(SeekFrom::Start(0)).unwrap();
        b.receive(&mut wire, buffers).unwrap();
        plan.acknowledge(i).unwrap();
    }
    plan.cursor()
}
fn expect(image: &DiskImage, block: u64, byte: u8, b: &mut Buffers) {
    b.block.fill(0xFE);
    image.read(block, &mut b.block).unwrap();
    assert!(
        b.block.iter().all(|v| *v == byte),
        "block {block} should contain {byte}"
    );
}
fn lazy_matches(
    index: &BlockIndex,
    volume: &Volume,
    image: &DiskImage,
    heads: &[CommitHash],
    b: &mut Buffers,
) {
    let mut actual = Box::new([0; BLOCK_BYTES]);
    for block in 0..volume.blocks() {
        index.read_at(volume, heads, block, &mut b.block).unwrap();
        image.read(block, &mut actual).unwrap();
        assert_eq!(b.block[..], actual[..], "block {block}");
    }
}

#[test]
fn overlap_converges_and_revocation_falls_back_with_lazy_historical_reads() {
    let dir = TempDir::new().unwrap();
    let mut b = Buffers::new(173, 97);
    let mut a = open(&dir.path().join("a"), 1, &mut b);
    let mut other = open(&dir.path().join("b"), 2, &mut b);
    write(&mut a, &[0], 0x11, None, &mut b);
    let ab = sync(&a, &mut other, &ExportCursor::new(), &mut b);
    let ha = write(&mut a, &[0, 1], 0x22, None, &mut b);
    let hb = write(&mut other, &[0, 2], 0x33, None, &mut b);
    sync(&a, &mut other, &ab, &mut b);
    sync(&other, &mut a, &ExportCursor::new(), &mut b);
    assert_eq!(
        a.ledger().heads(Heads::Admitted),
        other.ledger().heads(Heads::Admitted)
    );
    let mut ia = DiskImage::open(dir.path().join("ia"), 16, Follow::Working, &mut b).unwrap();
    let mut ib = DiskImage::open(dir.path().join("ib"), 16, Follow::Working, &mut b).unwrap();
    ia.update(&a, 1, &mut b).unwrap();
    ib.update(&other, 2, &mut b).unwrap();
    expect(&ia, 0, if ha > hb { 0x22 } else { 0x33 }, &mut b);
    for block in 0..16 {
        ia.read(block, &mut b.block).unwrap();
        let mut bytes = [0; BLOCK_BYTES];
        ib.read(block, &mut bytes).unwrap();
        assert_eq!(*b.block, bytes);
    }
    let mut index = BlockIndex::open(&dir.path().join("lazy.sqlite"), 16, Follow::Working).unwrap();
    index.update(&a, 1).unwrap();
    lazy_matches(&index, &a, &ia, &a.ledger().heads(Heads::Delivered), &mut b);
    index.read_at(&a, &[hb], 1, &mut b.block).unwrap();
    assert!(b.block.iter().all(|v| *v == 0));

    a.set_permissions(WritePermissions::from([(
        author(1),
        WritePermission::Unbounded,
    )]))
    .unwrap();
    ia.update(&a, 1, &mut b).unwrap();
    index.update(&a, 1).unwrap();
    expect(&ia, 0, 0x22, &mut b);
    expect(&ia, 2, 0, &mut b);
    lazy_matches(&index, &a, &ia, &a.ledger().heads(Heads::Delivered), &mut b);
    trusted(&mut a);
    let newer = write(&mut a, &[0], 0x44, None, &mut b);
    ia.update(&a, 1, &mut b).unwrap();
    expect(&ia, 0, 0x44, &mut b);
    assert!(a.ledger().is_ancestor(&hb, &newer));
    a.set_permissions(WritePermissions::from([
        (author(1), WritePermission::Bounded(vec![ha])),
        (author(2), WritePermission::Unbounded),
    ]))
    .unwrap();
    ia.update(&a, 1, &mut b).unwrap();
    expect(&ia, 0, if ha > hb { 0x22 } else { 0x33 }, &mut b);
}

#[test]
fn independent_pinned_branches_and_partial_resume_after_reordered_rebuild() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("v");
    let mut b = Buffers::new(31, 13);
    let mut v = open(&path, 1, &mut b);
    let root = write(&mut v, &[0], 1, None, &mut b);
    let left = write(&mut v, &[0, 1], 2, Some(vec![root]), &mut b);
    let right = write(&mut v, &[0, 2], 3, Some(vec![root]), &mut b);
    let mut l = DiskImage::open(
        dir.path().join("left"),
        16,
        Follow::Pinned(vec![left]),
        &mut b,
    )
    .unwrap();
    let mut r = DiskImage::open(
        dir.path().join("right"),
        16,
        Follow::Pinned(vec![right]),
        &mut b,
    )
    .unwrap();
    assert!(!l.step(&v, 1, &mut b).unwrap());
    assert!(l.checkpoint().is_partial());
    r.update(&v, 2, &mut b).unwrap();
    expect(&r, 0, 3, &mut b);
    let child = write(&mut v, &[0], 4, Some(vec![left]), &mut b);
    drop(l);
    drop(r);
    drop(v);
    let v = open(&path, 1, &mut b); // wire-hash archive order differs from arrival
    let mut l = DiskImage::open(dir.path().join("left"), 16, Follow::Working, &mut b).unwrap();
    let mut r = DiskImage::open(dir.path().join("right"), 16, Follow::Working, &mut b).unwrap();
    assert_eq!(l.checkpoint().follow(), &Follow::Pinned(vec![left]));
    l.update(&v, 1, &mut b).unwrap();
    expect(&l, 0, 2, &mut b);
    l.set_follow(Follow::Pinned(vec![child]));
    l.update(&v, 1, &mut b).unwrap();
    expect(&l, 0, 4, &mut b);
    r.update(&v, 1, &mut b).unwrap();
    expect(&r, 0, 3, &mut b);
    let mut index = BlockIndex::open(&dir.path().join("lazy.sqlite"), 16, Follow::Working).unwrap();
    index.update(&v, 1).unwrap();
    lazy_matches(&index, &v, &l, &[child], &mut b);
    lazy_matches(&index, &v, &r, &[right], &mut b);
}

#[test]
fn application_rejection_is_permanent_and_does_not_partially_install_a_write() {
    let dir = TempDir::new().unwrap();
    let mut b = Buffers::new(61, 17);
    let mut v = open(&dir.path().join("v"), 1, &mut b);
    write(&mut v, &[0], 7, None, &mut b);
    let mut bad = Vec::new();
    for (count, indices, body) in [
        (2, vec![0u64, 0], 2 * BLOCK_BYTES),
        (1, vec![16], BLOCK_BYTES),
        (1, vec![0], 2),
        (u64::MAX, vec![], 0),
    ] {
        let mut raw = count.to_le_bytes().to_vec();
        for index in indices {
            raw.extend(index.to_le_bytes());
        }
        raw.resize(raw.len() + body, 0x99);
        bad.push(v.commit_raw(&mut Cursor::new(raw), None, &mut b).unwrap());
    }
    let valid = write(&mut v, &[1], 8, None, &mut b); // child of malformed history still works
    let image_path = dir.path().join("image");
    let mut image = DiskImage::open(&image_path, 16, Follow::Working, &mut b).unwrap();
    image.update(&v, 2, &mut b).unwrap();
    expect(&image, 0, 7, &mut b);
    expect(&image, 1, 8, &mut b);
    let rejected: Vec<_> = image.checkpoint().rejected().map(|(h, _)| h).collect();
    for h in bad {
        assert!(rejected.contains(&h));
    }
    assert!(!rejected.contains(&valid));
    drop(image);
    let mut image = DiskImage::open(&image_path, 16, Follow::Working, &mut b).unwrap();
    assert!(image.is_current(&v));
    v.set_permissions(WritePermissions::new()).unwrap();
    image.update(&v, 1, &mut b).unwrap();
    expect(&image, 0, 0, &mut b);
    expect(&image, 1, 0, &mut b);
    trusted(&mut v);
    image.update(&v, 1, &mut b).unwrap();
    expect(&image, 0, 7, &mut b);
}

#[test]
fn redo_recovery_couples_image_index_and_checkpoint_at_every_persistence_boundary() {
    for fault in [
        Fault::BeforeJournalSync,
        Fault::AfterJournalSync,
        Fault::AfterIndexCommit,
        Fault::DuringImage,
        Fault::AfterImageSync,
    ] {
        let dir = TempDir::new().unwrap();
        let mut b = Buffers::new(113, 41);
        let mut v = open(&dir.path().join("v"), 1, &mut b);
        let root = write(&mut v, &[0], 0x11, None, &mut b);
        let path = dir.path().join("image");
        let mut image = DiskImage::open(&path, 16, Follow::Working, &mut b).unwrap();
        image.update(&v, 100, &mut b).unwrap();
        let next = write(&mut v, &[0, 1], 0x77, None, &mut b);
        assert!(image.step_with_fault(&v, 100, &mut b, fault).is_err());
        assert!(
            image.read(0, &mut b.block).is_err(),
            "poisoned image must not expose torn bytes"
        );
        assert_eq!(image.checkpoint().heads(), &[root]);
        drop(image);
        let mut recovered = DiskImage::open(&path, 16, Follow::Working, &mut b).unwrap();
        let committed = matches!(
            fault,
            Fault::AfterIndexCommit | Fault::DuringImage | Fault::AfterImageSync
        );
        expect(&recovered, 0, if committed { 0x77 } else { 0x11 }, &mut b);
        expect(&recovered, 1, if committed { 0x77 } else { 0 }, &mut b);
        assert_eq!(
            recovered.checkpoint().heads(),
            &[if committed { next } else { root }]
        );
        recovered.update(&v, 100, &mut b).unwrap();
        expect(&recovered, 0, 0x77, &mut b);
        expect(&recovered, 1, 0x77, &mut b);
        assert!(!path.join("journal").exists());
    }
}

#[test]
fn io_failure_rolls_back_instead_of_becoming_an_application_rejection() {
    let dir = TempDir::new().unwrap();
    let mut b = Buffers::new(101, 53);
    let path = dir.path().join("v");
    let mut v = open(&path, 1, &mut b);
    write(&mut v, &[0], 7, None, &mut b);
    let mut image = DiskImage::open(dir.path().join("image"), 16, Follow::Working, &mut b).unwrap();
    File::options()
        .write(true)
        .open(path.join("payloads"))
        .unwrap()
        .set_len(0)
        .unwrap();
    assert!(image.step(&v, 1, &mut b).is_err());
    assert!(image.checkpoint().heads().is_empty());
    assert_eq!(image.checkpoint().rejected().len(), 0);
}

#[test]
fn bounded_parking_duplicates_eviction_and_recovery_reuse_canonical_spool() {
    let dir = TempDir::new().unwrap();
    let mut b = Buffers::new(257, 131);
    let mut a = open(&dir.path().join("a"), 1, &mut b);
    write(&mut a, &[0], 1, None, &mut b);
    let mut first = a.ledger().export(&ExportCursor::new());
    let desc = a.ledger().export_item(&first, 0).unwrap();
    let mut root_wire = tempfile::tempfile().unwrap();
    a.export_item(&desc, &mut root_wire, &mut b).unwrap();
    first.acknowledge(0).unwrap();
    write(&mut a, &[0], 2, None, &mut b);
    let plan = a.ledger().export(&first.cursor());
    let mut child_wire = tempfile::tempfile().unwrap();
    a.export_item(
        &a.ledger().export_item(&plan, 0).unwrap(),
        &mut child_wire,
        &mut b,
    )
    .unwrap();
    let path = dir.path().join("b");
    let mut receiver = Volume::open_with_capacity(&path, key(2), 16, 1, &mut b).unwrap();
    for expected in ["waiting", "evicted"] {
        child_wire.seek(SeekFrom::Start(0)).unwrap();
        let result = receiver.receive(&mut child_wire, &mut b).unwrap();
        assert!(matches!(
            (expected, result),
            ("waiting", Received::Waiting(_)) | ("evicted", Received::Evicted { .. })
        ));
        assert_eq!(receiver.staged_items(), 1);
    }
    root_wire.seek(SeekFrom::Start(0)).unwrap();
    receiver.receive(&mut root_wire, &mut b).unwrap();
    assert_eq!(receiver.staged_items(), 0);
    let length = std::fs::metadata(path.join("payloads")).unwrap().len();
    drop(receiver);
    let receiver = Volume::open_with_capacity(&path, key(2), 16, 1, &mut b).unwrap();
    assert_eq!(
        receiver.ledger().heads(Heads::Admitted),
        a.ledger().heads(Heads::Admitted)
    );
    assert_eq!(
        std::fs::metadata(path.join("payloads")).unwrap().len(),
        length,
        "rebuild must not append decoded copies"
    );
}

#[test]
fn framing_decoder_handles_single_byte_buffers_and_rejects_truncation_and_trailing_data() {
    use stockfoil::DecodeStatus;
    let body = [
        3u64.to_le_bytes().as_slice(),
        b"abc",
        0u64.to_le_bytes().as_slice(),
    ]
    .concat();
    let mut d = BodyDecoder::new(2);
    let mut offset = 0;
    let mut decoded = Vec::new();
    let mut ends = 0;
    loop {
        let end = (offset + 1).min(body.len());
        let mut out = [0; 1];
        let (consumed, produced, status) = d
            .step(&body[offset..end], end == body.len(), &mut out)
            .unwrap();
        offset += consumed;
        decoded.extend_from_slice(&out[..produced]);
        if status == DecodeStatus::EndMember {
            ends += 1;
        }
        if status == DecodeStatus::Finished {
            break;
        }
    }
    assert_eq!(decoded, b"abc");
    assert_eq!(ends, 2);
    assert!(
        BodyDecoder::new(1)
            .step(&[1, 2], true, &mut [0; 1])
            .is_err()
    );
    assert!(BodyDecoder::new(0).step(&[1], true, &mut [0; 1]).is_err());
}

#[test]
fn real_fragment_framing_streams_multiple_members_and_rebuilds() {
    let dir = TempDir::new().unwrap();
    let mut b = Buffers::new(11, 7);
    let mut a = open(&dir.path().join("a"), 1, &mut b);
    for block in 0..3 {
        write(&mut a, &[block], block as u8 + 1, None, &mut b);
    }
    let mut raw = [1u64.to_le_bytes().as_slice(), 3u64.to_le_bytes().as_slice()].concat();
    raw.resize(16 + BLOCK_BYTES, 0xA5);
    for nonce in 0u64.. {
        raw[16..24].copy_from_slice(&nonce.to_le_bytes());
        let mut builder = a.ledger().begin_commit(None).unwrap();
        builder.update(&raw);
        if builder.finish().hash.as_slice()[0] == 0 {
            break;
        }
    }
    a.commit_raw(&mut Cursor::new(raw), None, &mut b).unwrap();
    let plan = a.ledger().export(&ExportCursor::new());
    assert!((0..plan.len()).any(|i| {
        let item = a.ledger().export_item(&plan, i).unwrap();
        matches!(item.id, stockfoil::ItemId::Fragment(_)) && item.members.len() > 1
    }));
    let path = dir.path().join("b");
    let mut other = open(&path, 2, &mut b);
    sync(&a, &mut other, &ExportCursor::new(), &mut b);
    drop(other);
    let other = open(&path, 2, &mut b);
    assert_eq!(
        other.ledger().heads(Heads::Admitted),
        a.ledger().heads(Heads::Admitted)
    );
    let mut image = DiskImage::open(dir.path().join("image"), 16, Follow::Working, &mut b).unwrap();
    image.update(&other, 1, &mut b).unwrap();
    for block in 0..3 {
        expect(&image, block, block as u8 + 1, &mut b);
    }
}

#[test]
fn corrupted_committed_journal_fails_closed_and_handles_are_exclusive() {
    use std::os::unix::fs::FileExt;
    let dir = TempDir::new().unwrap();
    let mut b = Buffers::new(67, 29);
    let volume_path = dir.path().join("v");
    let mut v = open(&volume_path, 1, &mut b);
    assert!(Volume::open(&volume_path, key(1), 16, &mut b).is_err());
    write(&mut v, &[0], 1, None, &mut b);
    let path = dir.path().join("image");
    let mut image = DiskImage::open(&path, 16, Follow::Working, &mut b).unwrap();
    assert!(DiskImage::open(&path, 16, Follow::Working, &mut b).is_err());
    assert!(
        image
            .step_with_fault(&v, 1, &mut b, Fault::AfterIndexCommit)
            .is_err()
    );
    drop(image);
    let journal = File::options()
        .write(true)
        .open(path.join("journal"))
        .unwrap();
    journal.write_all_at(&[0xFF], 30).unwrap();
    journal.sync_all().unwrap();
    drop(journal);
    assert!(DiskImage::open(&path, 16, Follow::Working, &mut b).is_err());
    assert!(
        path.join("journal").exists(),
        "a bad log must not be silently discarded"
    );
}

#[test]
#[ignore = "streams 2 GiB through commits, wire sync and an image; requires ~12 GiB free disk"]
fn multi_gib_streaming_has_flat_rss() {
    let dir = TempDir::new().unwrap();
    let mut b = Buffers::new(16384, 8192);
    let blocks = (2u64 << 30) / BLOCK_BYTES as u64;
    let indices: Vec<_> = (0..blocks).collect(); // caller's metadata, not payload bytes
    let mut a = Volume::open(dir.path().join("a"), key(1), blocks, &mut b).unwrap();
    let mut other = Volume::open(dir.path().join("b"), key(2), blocks, &mut b).unwrap();
    let before = rss_high_water();
    write(&mut a, &indices, 0xAB, None, &mut b);
    sync(&a, &mut other, &ExportCursor::new(), &mut b);
    let mut image =
        DiskImage::open(dir.path().join("image"), blocks, Follow::Working, &mut b).unwrap();
    trusted(&mut other);
    image.update(&other, 1, &mut b).unwrap();
    expect(&image, blocks - 1, 0xAB, &mut b);
    let growth = rss_high_water().saturating_sub(before);
    eprintln!("2 GiB: RSS high-water growth {growth} KiB");
    assert!(
        growth < 32 * 1024,
        "payload/index memory must not scale with 2 GiB of bytes"
    );
}
fn rss_high_water() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")
                .map(|v| v.split_whitespace().next().unwrap().parse().unwrap())
        })
        .unwrap()
}
