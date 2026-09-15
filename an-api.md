# API

Possible APIs for some different usecases.

## Automerge

```rust
struct Automerge {
    commit_graph: stockfoil::CommitGraph<Change>,
    opset: OpSet,
}

impl stockfoil::Commit for Change {

}

enum AmFragment {
    LooseCommit(storage::Change),
    Bundle(storage::Bundle),
}

impl stockfoil::Fragment for AmFragment {
    fn load(frag: stockfoil::Fragment) -> Result<Self, Error> {
        Ok(match frag.level() {
            0 => AmFragment::LooseCommit(storage::Change::parse(frag.data)?),
            _ => AmFragment::Bundle(storage::Bundle::parse(frag.data)?)
        })
    }
}
```

### Load Untrusted


```rust
impl Automerge {
    fn load_incremental(&mut self, data: Vec<u8>) -> Result<(), Error> {
        let changes: Vec<stockfoil::Commit<Change>> = stockfoil::load(data)?;
        let new_changes: Vec<stockfoil::Commit<Change>> = self.commit_graph.ingest(changes)?;
        self.apply_changes_log_patches(new_changes, patch_log)?;
    }
}
```

### Load Trusted

```rust
impl Automerge {
    fn load_trusted(&mut self, data: Vec<u8>) -> Result<(), Error> {
        let fragments = stockfoil::load_fragments(data)?;

        // This won't work in a trusted world because we don't have the 
        // commit graph right?
        //
        // Actually no that's wrong, we just won't necessarily know the
        // hashes. Stockfoil would need to have some arbitrary way of
        // identifying a commit I guess. Maybe that isn't really stockfoils
        // job?

        let ready = self.commit_graph.ingest(fragments)?;
        for fragment in fragments {
            match fragment {
                AmFragment::Commit(commit) => self.apply_
            }
        }
    }
}
```
