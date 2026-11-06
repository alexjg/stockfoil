# Known gaps

## Performance

- [ ] **Canonical traversal readiness.** `Traversal::schedule`
  (`stockfoil/src/history/traversal.rs`) scans earlier pending frontiers per
  scheduled node: O(records × concurrent chains). Cold materialization of 1M
  commits measured 255 ms versus 30 ms in insertion order; causal-difference
  enumeration measured 131 ms for 500k commits. Profile a clock-based readiness
  test and compare with `cargo bench -p stockfoil --bench history`.
- [ ] **Cascading rejections.** `is_rejected` in `stockfoil/src/materialize.rs`
  checks ancestry against every cascading rejection for each planned record.
  Consider a tainted-history clock; test thousands of cascading rejections.
- [ ] **Export planning.** Scans the inventory and cursor, then sorts outgoing
  items. Retirement handles standalone items and nested fragments, not general
  cover minimization.
- [ ] **Signed-draft scanning.** Rechecks every blocked signed draft on each
  signature or admission; quadratic with many blocked drafts.

## Limits and durability

- [ ] **Fragment size versus receiver limits.** Depth-3 fragments average about
  16 M members, beyond the default `Limits` of 2^20. Long histories need larger
  limits or a policy retaining smaller items when a covering fragment is too large.
- [ ] **Resource policy beyond per-item limits.** Aggregate quotas across
  concurrent admissions; budgets for placement and publication, which are
  unbudgeted and can walk substantial dependency history.
- [ ] **Draft durability.** Unsigned drafts live only in memory and cannot be
  recovered from exported items. Long-lived offline edits need a local journal.
- [ ] **Persistence review.** Review `document.rs` parked-item staging and the
  volume example's SQLite redo protocol beyond their existing tests.

## Upstream: Subduction

Recheck minimization reports against the current dependency before fixing upstream.

- [ ] 65,535-checkpoint limit per fragment.
- [ ] Minimization keeps offering loose commits below a signed fragment head.
- [ ] Minimization treats the range below a boundary as covered even for
  standalone commits of nonzero depth.
