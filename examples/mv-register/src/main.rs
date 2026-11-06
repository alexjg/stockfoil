//! Two replicas write concurrently, synchronize through `subduction_tokio`
//! (`load_and_watch` and `store_built_batch`), and then one revokes the
//! other's later writes.
use sedimentree_core::id::SedimentreeId;
use std::collections::HashMap;
use stockfoil::WritePermission;
use stockfoil_mv_register::SubductionReplica;

const TREE: SedimentreeId = SedimentreeId::new([7; 32]);

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let mut alice = SubductionReplica::new(TREE).await?;
    let mut bob = SubductionReplica::new(TREE).await?;
    let everyone = HashMap::from([
        (alice.author(), WritePermission::Unbounded),
        (bob.author(), WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(everyone.clone())?;
    bob.set_write_permissions(everyone)?;

    // Bob writes offline, then Alice and Bob connect: `load_and_watch`
    // automatically syncs Bob's initial write to Alice on connect.
    let first = bob.set("title", "Hi").await?;
    let (alice_conn, bob_conn) = alice.connect(&mut bob).await?;
    assert_eq!(alice.get("title"), bob.get("title"));

    // Both write concurrently and sync over their Subduction connection.
    alice.set("title", "Hello").await?;
    bob.set("title", "Howdy").await?;
    alice.sync_with(&mut bob).await?;
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

    drop((alice_conn, bob_conn));
    let report_a = alice.close().await;
    let report_b = bob.close().await;
    assert!(!report_a.forced && !report_b.forced);
    Ok(())
}
