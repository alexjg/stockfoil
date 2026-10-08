//! Two replicas write concurrently, synchronize in process, and then one
//! revokes the other's later writes.
use std::collections::HashMap;
use stockfoil::WritePermission;
use stockfoil_mv_register::MvRegisterMap;

fn main() -> eyre::Result<()> {
    let mut alice = MvRegisterMap::new();
    let mut bob = alice.fork()?;
    let everyone = HashMap::from([
        (alice.author(), WritePermission::Unbounded),
        (bob.author(), WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(everyone.clone())?;
    bob.set_write_permissions(everyone)?;

    let first = bob.set("title", "Hi")?;
    alice.merge(&mut bob)?;
    alice.set("title", "Hello")?;
    bob.set("title", "Howdy")?;
    let sent = alice.sync_to(&mut bob)? + bob.sync_to(&mut alice)?;
    println!("exchanged {sent} items");
    println!("alice: title = {:?}", alice.get("title"));
    println!("bob:   title = {:?}", bob.get("title"));
    assert_eq!(alice.values(), bob.values());

    // Alice only accepts Bob's writes up to his first one. His later value is
    // hidden and the one it overwrote is not revived, because Alice's own write
    // superseded it.
    let mut restricted = alice.write_permissions().clone();
    restricted.insert(bob.author(), WritePermission::Bounded(vec![first]));
    alice.set_write_permissions(restricted)?;
    println!(
        "alice, bob bounded at his first write: title = {:?}",
        alice.get("title")
    );
    assert_eq!(alice.sync_to(&mut bob)? + bob.sync_to(&mut alice)?, 0);
    println!("in sync: nothing left to send either way");
    Ok(())
}
