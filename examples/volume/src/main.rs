use ed25519_dalek::SigningKey;
use std::{
    io::{self, Seek, SeekFrom},
    path::PathBuf,
};
use stockfoil::{Author, ExportCursor, Follow, Heads, WritePermission, WritePermissions};
use stockfoil_volume::{BlockIndex, Buffers, DiskImage, Result, Volume};

fn author(seed: u8) -> Author {
    Author::try_from(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .as_bytes()
            .as_slice(),
    )
    .unwrap()
}
fn main() -> Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("volume-demo"));
    let mut buffers = Buffers::new(16 * 1024, 8 * 1024);
    let mut alice = Volume::open(
        root.join("alice"),
        SigningKey::from_bytes(&[1; 32]),
        1024,
        &mut buffers,
    )?;
    let mut bob = Volume::open(
        root.join("bob"),
        SigningKey::from_bytes(&[2; 32]),
        1024,
        &mut buffers,
    )?;
    let permissions = WritePermissions::from([
        (author(1), WritePermission::Unbounded),
        (author(2), WritePermission::Unbounded),
    ]);
    alice.set_permissions(permissions.clone())?;
    bob.set_permissions(permissions)?;
    let first = alice.write(&[0, 7], &mut io::repeat(0x11), None, &mut buffers)?;
    transfer(&alice, &mut bob, &root, &mut buffers)?;
    let left = alice.write(&[7], &mut io::repeat(0x22), None, &mut buffers)?;
    let right = bob.write(&[7, 9], &mut io::repeat(0x33), None, &mut buffers)?;
    transfer(&alice, &mut bob, &root, &mut buffers)?;
    transfer(&bob, &mut alice, &root, &mut buffers)?;

    let mut image = DiskImage::open(root.join("working"), 1024, Follow::Working, &mut buffers)?;
    image.set_follow(Follow::Working);
    image.update(&alice, 2, &mut buffers)?;
    image.read(7, &mut buffers.block)?;
    println!(
        "converged block 7: {:#04x} (concurrent winner is the larger hash)",
        buffers.block[0]
    );
    assert_eq!(buffers.block[0], if left > right { 0x22 } else { 0x33 });

    let mut branch = DiskImage::open(
        root.join("branch"),
        1024,
        Follow::Pinned(vec![first]),
        &mut buffers,
    )?;
    branch.set_follow(Follow::Pinned(vec![first]));
    branch.update(&alice, 1, &mut buffers)?;
    branch.read(7, &mut buffers.block)?;
    assert_eq!(buffers.block[0], 0x11);

    alice.set_permissions(WritePermissions::from([(
        author(1),
        WritePermission::Unbounded,
    )]))?;
    image.update(&alice, 2, &mut buffers)?;
    image.read(7, &mut buffers.block)?;
    assert_eq!(buffers.block[0], 0x22);
    println!("after hiding Bob: block 7 = 0x22; branch remains 0x11");

    let mut index = BlockIndex::open(&root.join("lazy.sqlite"), 1024, Follow::Working)?;
    index.update(&alice, 2)?;
    index.read_at(
        &alice,
        &alice.ledger().heads(Heads::Delivered),
        7,
        &mut buffers.block,
    )?;
    assert_eq!(buffers.block[0], 0x22);
    println!(
        "lazy read matches; sparse images and recoverable checkpoints in {}",
        root.display()
    );
    Ok(())
}
fn transfer(
    from: &Volume,
    to: &mut Volume,
    root: &std::path::Path,
    buffers: &mut Buffers,
) -> Result<()> {
    // A real transport would persist a per-peer cursor and acknowledge after
    // durable acceptance. Re-sending here also demonstrates idempotent receive.
    let plan = from.ledger().export(&ExportCursor::new());
    for i in 0..plan.len() {
        let item = from.ledger().export_item(&plan, i)?;
        let path = root.join("transport.tmp");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)?;
        from.export_item(&item, &mut file, buffers)?;
        file.seek(SeekFrom::Start(0))?;
        to.receive(&mut file, buffers)?;
        std::fs::remove_file(path)?;
    }
    Ok(())
}
