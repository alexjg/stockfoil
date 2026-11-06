//! Offline edits, synchronization, and a writer's bounded permissions.
use stockfoil::{WritePermission, WritePermissions};
use stockfoil_mv_register::MvRegisterMap;

fn main() -> eyre::Result<()> {
    let mut alice = MvRegisterMap::new();
    let mut bob = MvRegisterMap::new();
    let everyone = WritePermissions::from([
        (alice.author(), WritePermission::Unbounded),
        (bob.author(), WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(everyone.clone())?;
    bob.set_write_permissions(everyone)?;

    let first = bob.set("title", "Hi")?;
    bob.sync_to(&mut alice)?;

    // Neither edit has seen the other, so both values survive synchronization.
    alice.set("title", "Hello")?;
    bob.set("title", "Howdy")?;
    alice.sync_to(&mut bob)?;
    bob.sync_to(&mut alice)?;
    println!("alice: title = {:?}", alice.get("title"));
    println!("bob:   title = {:?}", bob.get("title"));
    assert_eq!(alice.values(), bob.values());

    let mut restricted = alice.write_permissions().clone();
    restricted.insert(bob.author(), WritePermission::Bounded(vec![first]));
    alice.set_write_permissions(restricted)?;
    println!(
        "alice, bob bounded at his first write: title = {:?}",
        alice.get("title")
    );
    Ok(())
}
