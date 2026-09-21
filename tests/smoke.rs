use std::{collections::{BTreeSet}};

mod mv_register;
mod orset;
mod support;

#[test]
fn test_basic() {
    let mut doc = orset::OrSet::new();
    doc.add("foo".to_string());

    assert_eq!(doc.value(), BTreeSet::from_iter(["foo".to_string()]));

    doc.remove("foo".to_string());
    assert_eq!(doc.value(), BTreeSet::new());

    let mut doc2 = doc.fork();
    doc2.add("bar".to_string());
    assert_eq!(doc2.value(), BTreeSet::from_iter(["bar".to_string()]));

    doc2.remove("bar".to_string());
    assert_eq!(doc2.value(), BTreeSet::new());

    doc.add("bar".to_string());
    doc2.merge(&doc);

    assert_eq!(doc2.value(), BTreeSet::from_iter(["bar".to_string()]));
}
