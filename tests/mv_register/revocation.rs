//! Red contract test for the API in design/authors_signatures_api.md.
//! These APIs intentionally do not exist yet; don't replace them with mocks.

use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
};

use stockfoil::{Vis, VisDiff, WritePermission};

use super::MvRegisterMap;

fn values(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn revocation_changes_participation_without_retracting_delivery() {
    let mut alice = MvRegisterMap::new();
    let alice_id = alice.author();
    alice.set_write_permissions(&HashMap::from([(alice_id, WritePermission::Unbounded)]));
    let original = alice.set("title", "original");
    let mut bob = alice.fork();
    let bob_id = bob.author();
    let allowed = HashMap::from([
        (alice_id, WritePermission::Unbounded),
        (bob_id, WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(&allowed);
    bob.set_write_permissions(&allowed);

    // Two concurrent branches. The bound will include Alice's first write,
    // but neither of her later writes.
    let bound = alice.set("title", "allowed");
    let alice_branch = alice.graph.perspective();
    let concurrent = bob.set("title", "concurrent");
    let bob_branch = bob.graph.perspective();
    let revoked_title = alice.set("title", "revoked");
    let revoked_key = alice.set("private", "hidden after revocation");

    bob.merge(&alice);
    let descendant = bob.set("description", "still here");
    alice.merge(&bob);
    assert!(alice.graph.is_ancestor(&revoked_key, &descendant));
    assert_eq!(alice.get("title"), values(&["revoked", "concurrent"]));
    assert_eq!(alice.get("private"), values(&["hidden after revocation"]));
    assert_eq!(alice.get("description"), values(&["still here"]));

    // A diff isn't necessarily forwards or backwards: these branches need both.
    let branches = alice.graph.diff(&alice_branch, &bob_branch).unwrap();
    assert_eq!(branches.visibility(&original).unwrap(), Vis::Both);
    assert_eq!(branches.visibility(&bound).unwrap(), Vis::Before);
    assert_eq!(branches.visibility(&concurrent).unwrap(), Vis::After);
    assert_eq!(branches.visibility(&revoked_title).unwrap(), Vis::Neither);
    let branch_changes = branches.changed_commits().collect::<Vec<_>>();
    assert_eq!(branch_changes.len(), 2);
    assert!(
        branch_changes.iter().any(|change| {
            change.commit_id == bound && change.visibility_delta == VisDiff::Before
        })
    );
    assert!(branch_changes.iter().any(|change| {
        change.commit_id == concurrent && change.visibility_delta == VisDiff::After
    }));

    let before = alice.graph.perspective();
    let restricted = HashMap::from([
        (alice_id, WritePermission::Bounded(vec![bound])),
        (bob_id, WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(&restricted);
    let after = alice.graph.perspective();

    // Revocation restores the overwritten value, removes the new key, and
    // leaves both the concurrent write and Bob's allowed descendant alone.
    assert_eq!(alice.get("title"), values(&["allowed", "concurrent"]));
    assert!(alice.get("private").is_empty());
    assert_eq!(alice.get("description"), values(&["still here"]));
    assert!(before.delivered_heads() == after.delivered_heads());
    let mut redelivered = 0;
    alice
        .graph
        .traverse_delivered_since(before.delivered_heads(), |_, _| {
            redelivered += 1;
            ControlFlow::Continue(())
        });
    assert_eq!(redelivered, 0);

    let diff = alice.graph.diff(&before, &after).unwrap();
    let changes = diff.changed_commits().collect::<Vec<_>>();
    assert_eq!(changes.len(), 2);
    assert!(
        changes
            .iter()
            .all(|c| c.visibility_delta == VisDiff::Before)
    );
    assert!(
        changes.iter().map(|c| c.commit_id).collect::<BTreeSet<_>>()
            == BTreeSet::from([revoked_title, revoked_key])
    );
    // The bound participates in BOTH perspectives, but becomes a visible
    // register value only after its overwriter stops participating.
    assert_eq!(diff.visibility(&bound).unwrap(), Vis::Both);
    assert_eq!(diff.visibility(&descendant).unwrap(), Vis::Both);
    assert_eq!(diff.visibility(&revoked_title).unwrap(), Vis::Before);
    let reverse = alice.graph.diff(&after, &before).unwrap();
    let additions = reverse.changed_commits().collect::<Vec<_>>();
    assert_eq!(additions.len(), 2);
    assert!(
        additions
            .iter()
            .all(|c| c.visibility_delta == VisDiff::After)
    );
    assert!(
        additions
            .iter()
            .map(|c| c.commit_id)
            .collect::<BTreeSet<_>>()
            == BTreeSet::from([revoked_title, revoked_key])
    );
    assert_eq!(
        alice
            .graph
            .diff(&after, &after)
            .unwrap()
            .changed_commits()
            .count(),
        0
    );

    // Old perspectives don't change, and another replica can resolve them
    // without adopting their write permissions as its current policy.
    assert_eq!(
        alice.get_at(&before, "title").unwrap(),
        values(&["revoked", "concurrent"])
    );
    assert_eq!(
        bob.get_at(&after, "title").unwrap(),
        values(&["allowed", "concurrent"])
    );
    assert!(bob.get_at(&after, "private").unwrap().is_empty());
    assert_eq!(bob.get("title"), values(&["revoked", "concurrent"]));

    // Repeated delivery must not import Bob's unbounded permissions or undo
    // Alice's revocation. Adopting the same permissions gives the same state.
    alice.merge(&bob);
    assert_eq!(alice.get("title"), values(&["allowed", "concurrent"]));
    assert!(alice.get("private").is_empty());
    bob.set_write_permissions(&restricted);
    bob.merge(&alice);
    assert_eq!(bob.get("title"), alice.get("title"));
    assert_eq!(bob.get("description"), alice.get("description"));
    assert!(bob.get("private").is_empty());

    // Replacing the map really replaces it: omitting Alice denies even her
    // earlier commits. Explicit denial and an empty bound mean the same thing.
    alice.set_write_permissions(&HashMap::from([(bob_id, WritePermission::Unbounded)]));
    let omitted = alice.graph.perspective();
    assert_eq!(alice.get("title"), values(&["concurrent"]));
    assert_eq!(alice.get("description"), values(&["still here"]));
    let mut denied = HashMap::from([
        (alice_id, WritePermission::Denied),
        (bob_id, WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(&denied);
    assert_eq!(
        alice
            .graph
            .diff(&omitted, &alice.graph.perspective())
            .unwrap()
            .changed_commits()
            .count(),
        0
    );
    denied.insert(alice_id, WritePermission::Bounded(vec![]));
    alice.set_write_permissions(&denied);
    assert_eq!(
        alice
            .graph
            .diff(&omitted, &alice.graph.perspective())
            .unwrap()
            .changed_commits()
            .count(),
        0
    );
    assert_eq!(alice.get("title"), values(&["concurrent"]));

    // Restoring permission needs no commit replay or new signatures.
    alice.set_write_permissions(&allowed);
    assert_eq!(alice.get("title"), values(&["revoked", "concurrent"]));
    assert_eq!(alice.get("private"), values(&["hidden after revocation"]));
    assert!(alice.graph.perspective().delivered_heads() == before.delivered_heads());
    assert_eq!(
        alice.get_at(&after, "title").unwrap(),
        values(&["allowed", "concurrent"])
    );
    assert!(alice.get_at(&after, "private").unwrap().is_empty());

    // Bob can overwrite through the revoked history. Making Alice's commits
    // participate again must not bring back values superseded by Bob.
    bob.set("title", "descendant wins");
    assert_eq!(bob.get("title"), values(&["descendant wins"]));
    bob.set_write_permissions(&allowed);
    assert_eq!(bob.get("title"), values(&["descendant wins"]));

    // Unbounded isn't an alias for today's heads; it permits future writes.
    // That doesn't expand an already captured perspective.
    alice.set("title", "future write");
    assert_eq!(alice.get("title"), values(&["future write"]));
    assert_eq!(
        alice.get_at(&before, "title").unwrap(),
        values(&["revoked", "concurrent"])
    );
    assert_eq!(
        alice.get_at(&after, "title").unwrap(),
        values(&["allowed", "concurrent"])
    );
}
