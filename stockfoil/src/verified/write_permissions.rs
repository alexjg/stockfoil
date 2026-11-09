//! The public permission collection, including the rule for unlisted authors.
use super::{author::Author, write_permission::WritePermission};
use std::collections::HashMap;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::{prelude::*, std_specs::iter::IteratorSpec};

verus! {

broadcast use {vstd::seq::group_seq_lemmas, vstd::seq_lib::group_seq_properties, vstd::seq_lib::group_seq_lib_default};

/// Per-author write permissions, plus the rule for authors not listed.
///
/// Unlisted authors are [`Denied`](WritePermission::Denied) unless
/// [`with_unlisted`](Self::with_unlisted) says otherwise. A listed author's own
/// rule always takes precedence.
#[cfg_attr(not(verus_keep_ghost), derive(Debug))]
pub struct WritePermissions {
    authors: HashMap<Author, WritePermission>,
    unlisted: WritePermission,
}

impl Clone for WritePermissions {
    fn clone(&self) -> (out: Self)
        ensures out.same(self),
    {
        let authors = self.authors.clone();
        let unlisted = self.unlisted.clone();
        proof {
            assert forall|a: Author| authors@.contains_key(a) implies authors@[a].same(&self.authors@[a]) by {
                assert(vstd::pervasive::cloned(self.authors@[a], authors@[a]));
            }
        }
        Self { authors, unlisted }
    }
}

impl Default for WritePermissions {
    fn default() -> (out: Self)
        ensures out.authors() == Map::<Author, WritePermission>::empty(), out.fallback() is Denied,
    { Self { authors: HashMap::new(), unlisted: WritePermission::Denied } }
}

impl WritePermissions {
    pub proof fn same_comparison(&self, a: &Self, b: &Self)
        requires a.same(b),
        ensures self.same(a) == self.same(b),
    {
        assert forall|key: Author| self.authors().contains_key(key) && a.authors().contains_key(key)
            implies self.authors()[key].same(&a.authors()[key]) == self.authors()[key].same(&b.authors()[key]) by {}
    }

    pub open spec fn same(&self, other: &Self) -> bool {
        self.authors().dom() == other.authors().dom() && self.fallback().same(&other.fallback())
            && forall|a: Author| self.authors().contains_key(a) ==> self.authors()[a].same(&other.authors()[a])
    }

    pub closed spec fn authors(&self) -> Map<Author, WritePermission> { self.authors@ }
    pub closed spec fn fallback(&self) -> WritePermission { self.unlisted }
    pub open spec fn rule(&self, author: Author) -> WritePermission {
        if self.authors().contains_key(author) { self.authors()[author] } else { self.fallback() }
    }

    /// No listed authors; unlisted authors are denied.
    pub fn new() -> (out: Self)
        ensures out.authors() == Map::<Author, WritePermission>::empty(), out.fallback() is Denied,
    { Self::default() }

    /// The serialization order is byte order, independent of hash-map insertion order.
    pub(crate) fn ordered_keys(&self) -> (out: Vec<super::commit_hash::CommitHash>)
        ensures super::stdlib::hashes_sorted(out@), out@.no_duplicates(),
            forall|h: super::commit_hash::CommitHash| out@.contains(h) == self.authors().contains_key(Author(h.0)),
    {
        proof { super::stdlib::author_key_model(); }
        let mut keys: Vec<super::commit_hash::CommitHash> = Vec::new();
        let iterator = self.authors.iter();
        let ghost entries = iterator.remaining();
        for (author, _) in iter: iterator
            invariant iter.seq() == entries, vstd::std_specs::hash::obeys_key_model::<Author>(),
                forall|j: int| 0 <= j < iter.seq().len() ==> self.authors@.contains_key(*iter.seq()[j].0),
                forall|a: Author| self.authors@.contains_key(a) ==> iter.seq().contains((&a, &self.authors@[a])),
                forall|h: super::commit_hash::CommitHash| keys@.contains(h) ==> self.authors@.contains_key(Author(h.0)),
                forall|j: int| 0 <= j < iter.index() ==> keys@.contains(super::commit_hash::CommitHash(iter.seq()[j].0.0)),
        {
            let ghost before = keys@;
            keys.push(super::commit_hash::CommitHash(author.0));
            proof {
                assert forall|h: super::commit_hash::CommitHash| keys@.contains(h) implies self.authors@.contains_key(Author(h.0)) by {
                    if !before.contains(h) { assert(h == super::commit_hash::CommitHash(author.0)); }
                }
                assert forall|j: int| 0 <= j < iter.index()+1 implies keys@.contains(super::commit_hash::CommitHash(iter.seq()[j].0.0)) by {
                    if j < iter.index() { assert(before.contains(super::commit_hash::CommitHash(iter.seq()[j].0.0))); }
                    else { assert(iter.seq()[j].0 == author); }
                }
            }
        }
        proof {
            assert forall|h: super::commit_hash::CommitHash| self.authors@.contains_key(Author(h.0)) implies keys@.contains(h) by {
                let a = Author(h.0);
                let j = choose|j: int| 0 <= j < entries.len() && entries[j] == (&a, &self.authors@[a]);
            }
        }
        let ghost before = keys@;
        super::stdlib::normalize_hashes(&mut keys);
        assert(keys@.to_set() == before.to_set());
        proof { keys@.to_set_ensures(); before.to_set_ensures(); }
        assert forall|h: super::commit_hash::CommitHash| keys@.contains(h) == self.authors().contains_key(Author(h.0)) by {
            assert(keys@.to_set().contains(h) == before.to_set().contains(h));
            assert(keys@.contains(h) == before.contains(h));
        }
        keys
    }

    /// Set the rule for authors without their own entry.
    pub fn with_unlisted(mut self, permission: WritePermission) -> (out: Self)
        ensures out.authors() == self.authors(), out.fallback() == permission,
    { self.unlisted = permission; self }

    /// The rule for authors without their own entry.
    pub fn unlisted(&self) -> (out: &WritePermission)
        ensures *out == self.fallback(),
    { &self.unlisted }

    pub fn set_unlisted(&mut self, permission: WritePermission)
        ensures final(self).authors() == old(self).authors(), final(self).fallback() == permission,
    { self.unlisted = permission; }

    pub(crate) fn listed(&self, author: &Author) -> (out: Option<&WritePermission>)
        ensures match out {
            Some(p) => self.authors().contains_key(*author) && *p == self.authors()[*author],
            None => !self.authors().contains_key(*author),
        },
    {
        proof { super::stdlib::author_key_model(); }
        self.authors.get(author)
    }

    /// The rule that applies to `author`, listed or not.
    pub fn permission(&self, author: &Author) -> (out: &WritePermission)
        ensures *out == self.rule(*author),
    {
        match self.listed(author) { Some(p) => p, None => &self.unlisted }
    }

    /// Give `author` its own rule, returning any rule it replaces.
    pub fn insert(&mut self, author: Author, permission: WritePermission) -> (out: Option<WritePermission>)
        ensures final(self).authors() == old(self).authors().insert(author, permission),
            final(self).fallback() == old(self).fallback(),
            out == if old(self).authors().contains_key(author) { Some(old(self).authors()[author]) } else { None },
    {
        proof { super::stdlib::author_key_model(); }
        self.authors.insert(author, permission)
    }

    /// Remove `author`'s own rule, so the unlisted rule applies to it.
    pub fn remove(&mut self, author: &Author) -> (out: Option<WritePermission>)
        ensures final(self).authors() == old(self).authors().remove(*author),
            final(self).fallback() == old(self).fallback(),
            out == if old(self).authors().contains_key(*author) { Some(old(self).authors()[*author]) } else { None },
    {
        proof { super::stdlib::author_key_model(); }
        self.authors.remove(author)
    }
}

#[cfg(verus_keep_ghost)]
impl vstd::std_specs::cmp::PartialEqSpecImpl for WritePermissions {
    open spec fn obeys_eq_spec() -> bool { true }
    open spec fn eq_spec(&self, other: &Self) -> bool { self.same(other) }
}
impl Eq for WritePermissions {}

impl PartialEq for WritePermissions {
    fn eq(&self, other: &Self) -> (out: bool)
        ensures out == self.same(other),
    {
        proof { super::stdlib::author_key_model(); }
        if self.authors.len() != other.authors.len() || self.unlisted != other.unlisted { return false; }
        for (author, permission) in iter: self.authors.iter()
            invariant vstd::std_specs::hash::obeys_key_model::<Author>(), self.authors@.len() == other.authors@.len(), self.unlisted.same(&other.unlisted),
                forall|j: int| 0 <= j < iter.seq().len() ==> self.authors@.contains_key(*iter.seq()[j].0)
                    && self.authors@[*iter.seq()[j].0] == *iter.seq()[j].1,
                forall|a: Author| self.authors@.contains_key(a) ==> iter.seq().contains((&a, &self.authors@[a])),
                forall|j: int| 0 <= j < iter.index() ==> other.authors@.contains_key(*iter.seq()[j].0)
                    && iter.seq()[j].1.same(&other.authors@[*iter.seq()[j].0]),
        {
            match other.authors.get(author) {
                Some(other_permission) => { if permission != other_permission { return false; } },
                None => { return false; },
            }
        }
        proof {
            assert(self.authors@.dom().subset_of(other.authors@.dom()));
            vstd::set_lib::lemma_subset_equality(self.authors@.dom(), other.authors@.dom());
        }
        true
    }
}

} // verus!

#[cfg(not(verus_keep_ghost))]
impl WritePermissions {
    /// Listed authors and their rules, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (&Author, &WritePermission)> {
        self.authors.iter()
    }
}

#[cfg(not(verus_keep_ghost))]
impl FromIterator<(Author, WritePermission)> for WritePermissions {
    fn from_iter<I: IntoIterator<Item = (Author, WritePermission)>>(iter: I) -> Self {
        Self {
            authors: iter.into_iter().collect(),
            unlisted: WritePermission::Denied,
        }
    }
}

#[cfg(not(verus_keep_ghost))]
impl<const N: usize> From<[(Author, WritePermission); N]> for WritePermissions {
    fn from(authors: [(Author, WritePermission); N]) -> Self {
        authors.into_iter().collect()
    }
}
