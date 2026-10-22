//! Thin wasm-bindgen binding over the Stockfoil core.
//!
//! Every value crossing the boundary is owned: bytes are copied into fresh
//! `Uint8Array`s on the way out and into Rust buffers on the way in, so nothing
//! borrowed from Wasm memory survives a call. Hashes, authors, and signatures
//! are `Uint8Array`s (32/32/64 bytes); payload IDs and tickets are `BigInt`s.
//! The host loop, codecs, and storage live in the TypeScript wrapper.
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use stockfoil_core::{
    Author, BlobHash, BundleMeta, Change, CommitHash, Counts, DecodeNext, DecodeStatus, Delivered,
    Error, FormatId, Heads, ItemId, Limits, Next, ParentRef, PayloadId, Receipt, ReceiveOptions,
    Reply, ReplyError, Request, Signature, Ticket, Vis, WritePermission, WritePermissions,
};
use wasm_bindgen::prelude::*;

type Result<T> = std::result::Result<T, JsValue>;

// ---- errors ----

fn js_error(code: &str, message: &str) -> Object {
    let error = js_sys::Error::new(message);
    error.set_name("StockfoilError");
    set(&error, "code", code);
    error.into()
}

/// A `StockfoilError` whose `code` names the core error variant.
fn core_error(e: &Error) -> JsValue {
    let detail = match e {
        Error::InvalidPublicKey(author)
        | Error::InvalidSignature { author, .. }
        | Error::MissingSignature { author, .. } => Some(("author", bytes(author.as_slice()))),
        Error::MissingDependency(h)
        | Error::UnknownCommit(h)
        | Error::MissingParent(h)
        | Error::NotDraft(h)
        | Error::CorruptPayload(h) => Some(("hash", bytes(h.as_slice()))),
        _ => None,
    };
    let code = e.code();
    let error = js_error(code, &e.to_string());
    if let Some((name, value)) = detail {
        set(&error, name, value);
    }
    error.into()
}

fn invalid(message: &str) -> JsValue {
    js_error("InvalidInput", message).into()
}

fn core<T>(result: std::result::Result<T, Error>) -> Result<T> {
    result.map_err(|e| core_error(&e))
}

// ---- value conversion ----

fn set(object: &Object, key: &str, value: impl Into<JsValue>) {
    Reflect::set(object, &key.into(), &value.into()).expect("set on a plain object");
}

fn get(object: &JsValue, key: &str) -> JsValue {
    Reflect::get(object, &key.into()).unwrap_or(JsValue::UNDEFINED)
}

fn bytes(b: &[u8]) -> JsValue {
    Uint8Array::from(b).into()
}

fn fixed<const N: usize>(b: &[u8], what: &str) -> Result<[u8; N]> {
    b.try_into()
        .map_err(|_| invalid(&format!("{what} must be {N} bytes")))
}

fn commit_hash(b: &[u8]) -> Result<CommitHash> {
    Ok(CommitHash::from(fixed::<32>(b, "a commit hash")?))
}

fn author(b: &[u8]) -> Result<Author> {
    Ok(Author::try_from(fixed::<32>(b, "an author")?.as_slice()).expect("32 bytes"))
}

fn byte_value(value: &JsValue, what: &str) -> Result<Vec<u8>> {
    value
        .dyn_ref::<Uint8Array>()
        .map(Uint8Array::to_vec)
        .ok_or_else(|| invalid(&format!("{what} must be a Uint8Array")))
}

fn each(value: &JsValue, what: &str) -> Result<Vec<JsValue>> {
    let iter =
        js_sys::try_iter(value)?.ok_or_else(|| invalid(&format!("{what} must be iterable")))?;
    iter.collect()
}

fn hash_list(value: &JsValue, what: &str) -> Result<Vec<CommitHash>> {
    each(value, what)?
        .iter()
        .map(|h| commit_hash(&byte_value(h, "a commit hash")?))
        .collect()
}

fn hash_array(hashes: impl IntoIterator<Item = CommitHash>) -> Array {
    hashes.into_iter().map(|h| bytes(h.as_slice())).collect()
}

fn payload_id(value: &JsValue) -> Result<PayloadId> {
    u64::try_from(value.clone()).map_err(|_| invalid("payload IDs must be BigInts in u64 range"))
}

fn number(value: &JsValue, what: &str) -> Result<usize> {
    value
        .as_f64()
        .filter(|n| n.fract() == 0.0 && *n >= 0.0 && *n <= 9007199254740991.0)
        .map(|n| n as usize)
        .ok_or_else(|| invalid(&format!("{what} must be a non-negative integer")))
}

fn item_id(id: &ItemId) -> JsValue {
    let o = Object::new();
    match id {
        ItemId::Commit(h) => {
            set(&o, "kind", "commit");
            set(&o, "head", bytes(h.as_slice()));
        }
        ItemId::Fragment(r) => {
            set(&o, "kind", "fragment");
            set(&o, "head", bytes(r.head.as_slice()));
            set(&o, "boundary", hash_array(r.boundary.iter().copied()));
        }
    }
    o.into()
}

fn item_ids<'a>(ids: impl IntoIterator<Item = &'a ItemId>) -> Array {
    ids.into_iter().map(item_id).collect()
}

/// `{ id, members: [{ author, parents: (number | Uint8Array)[] }], evidence }`.
/// A numeric parent is an earlier member's index; bytes are an external hash.
fn meta(meta: &BundleMeta) -> JsValue {
    let o = Object::new();
    set(&o, "id", item_id(&meta.id));
    let members: Array = meta
        .shape
        .members
        .iter()
        .map(|m| {
            let member = Object::new();
            set(&member, "author", bytes(m.author.as_slice()));
            let parents: Array = m
                .parents
                .iter()
                .map(|p| match p {
                    ParentRef::Member(i) => JsValue::from(*i as f64),
                    ParentRef::External(h) => bytes(h.as_slice()),
                })
                .collect();
            set(&member, "parents", parents);
            JsValue::from(member)
        })
        .collect();
    set(&o, "members", members);
    let evidence: Array = meta
        .evidence
        .iter()
        .map(|e| {
            let ev = Object::new();
            set(&ev, "author", bytes(e.author.as_slice()));
            set(&ev, "member", e.member as f64);
            set(&ev, "signature", bytes(e.signature.as_slice()));
            JsValue::from(ev)
        })
        .collect();
    set(&o, "evidence", evidence);
    o.into()
}

fn delivered(d: &Delivered) -> JsValue {
    let o = Object::new();
    set(&o, "hash", bytes(d.hash.as_slice()));
    set(&o, "payload", d.payload);
    set(&o, "newlyDelivered", d.newly_delivered);
    o.into()
}

fn receipt(r: &Receipt) -> JsValue {
    let o = Object::new();
    set(&o, "id", item_id(&r.id));
    set(
        &o,
        "members",
        r.members.iter().map(delivered).collect::<Array>(),
    );
    set(&o, "draftsAdmitted", item_ids(&r.drafts_admitted));
    o.into()
}

fn request(ticket: Ticket, request: Request<'_>) -> JsValue {
    let r = Object::new();
    match request {
        Request::Read { offset, max } => {
            set(&r, "kind", "read");
            set(&r, "offset", offset as f64);
            set(&r, "max", max as f64);
        }
        Request::Decode {
            input,
            eof,
            capacity,
            ..
        } => {
            // The decoder's metadata is available once from `Admission.meta()`.
            set(&r, "kind", "decode");
            set(&r, "input", bytes(input));
            set(&r, "eof", eof);
            set(&r, "capacity", capacity as f64);
        }
        Request::Bind { meta } => {
            set(&r, "kind", "bind");
            set(&r, "memberCount", meta.len() as f64);
        }
    }
    let o = Object::new();
    set(&o, "type", "request");
    set(&o, "ticket", ticket.get());
    set(&o, "request", r);
    o.into()
}

fn next(next: Next<'_>) -> JsValue {
    let o = Object::new();
    match next {
        Next::Continue => set(&o, "type", "continue"),
        Next::Request(t, r) => return request(t, r),
        Next::AwaitReply => set(&o, "type", "awaitReply"),
        Next::NeedHistory => set(&o, "type", "needHistory"),
        Next::Waiting(h) => {
            set(&o, "type", "waiting");
            set(&o, "hash", bytes(h.as_slice()));
        }
        Next::Done(r) => {
            set(&o, "type", "done");
            set(&o, "receipt", receipt(r));
        }
        Next::Failed(e) => {
            set(&o, "type", "failed");
            set(&o, "error", core_error(e));
        }
    }
    o.into()
}

fn decode_next(next: DecodeNext<'_>) -> JsValue {
    let o = Object::new();
    match next {
        DecodeNext::Request(t, r) => return request(t, r),
        DecodeNext::AwaitReply => set(&o, "type", "awaitReply"),
        DecodeNext::Chunk { member, bytes: b } => {
            set(&o, "type", "chunk");
            set(&o, "member", member as f64);
            set(&o, "bytes", bytes(b));
        }
        DecodeNext::Done => set(&o, "type", "done"),
        DecodeNext::Failed(e) => {
            set(&o, "type", "failed");
            set(&o, "error", core_error(e));
        }
    }
    o.into()
}

fn receive_options(options: &JsValue) -> Result<ReceiveOptions> {
    let format = get(options, "format")
        .as_string()
        .ok_or_else(|| invalid("options.format must be a string"))?;
    let mut o = ReceiveOptions::new(format_id(&format)?);
    o.limits = limits(&get(options, "limits"))?;
    for (name, slot) in [
        ("readSize", &mut o.read_size),
        ("decodeCapacity", &mut o.decode_capacity),
        ("quantum", &mut o.quantum),
    ] {
        let v = get(options, name);
        if !v.is_undefined() {
            *slot = number(&v, name)?.max(1);
        }
    }
    Ok(o)
}

fn counts_js(c: Counts) -> JsValue {
    let o = Object::new();
    set(&o, "bytesRead", c.bytes_read as f64);
    set(&o, "encodedBytesDecoded", c.encoded_bytes_decoded as f64);
    set(&o, "canonicalBytes", c.canonical_bytes as f64);
    set(&o, "requests", c.requests as f64);
    o.into()
}

fn accept_reply(
    ticket: u64,
    reply: JsValue,
    f: impl FnOnce(Ticket, Reply<'_>) -> std::result::Result<(), ReplyError>,
) -> Result<()> {
    let ticket = Ticket::from_raw(ticket);
    let kind = get(&reply, "kind").as_string().unwrap_or_default();
    let result = match kind.as_str() {
        "read" => {
            let data = byte_value(&get(&reply, "bytes"), "reply.bytes")?;
            let eof = get(&reply, "eof").is_truthy();
            f(ticket, Reply::Read { bytes: &data, eof })
        }
        "decoded" => {
            let consumed = number(&get(&reply, "consumed"), "reply.consumed")?;
            let output = byte_value(&get(&reply, "output"), "reply.output")?;
            let status = match get(&reply, "status").as_string().as_deref() {
                Some("working") => DecodeStatus::Working,
                Some("needInput") => DecodeStatus::NeedInput,
                Some("endMember") => DecodeStatus::EndMember,
                Some("finished") => DecodeStatus::Finished,
                _ => return Err(invalid("reply.status must be a decode status")),
            };
            f(
                ticket,
                Reply::Decoded {
                    consumed,
                    output: &output,
                    status,
                },
            )
        }
        "bound" => {
            let ids = each(&get(&reply, "ids"), "reply.ids")?
                .iter()
                .map(payload_id)
                .collect::<Result<Vec<_>>>()?;
            f(ticket, Reply::Bound(&ids))
        }
        "failed" => {
            let message = get(&reply, "message")
                .as_string()
                .unwrap_or_else(|| "host request failed".into());
            f(ticket, Reply::Failed(&message))
        }
        _ => {
            return Err(invalid(
                "reply.kind must be read, decoded, bound, or failed",
            ));
        }
    };
    result.map_err(|e| match e {
        ReplyError::Stale => js_error("StaleReply", "stale or unknown reply ticket").into(),
        ReplyError::WrongKind => {
            js_error("WrongKindReply", "reply kind does not match the request").into()
        }
        ReplyError::Invalid(e) => core_error(&e),
    })
}

fn permissions(value: &JsValue) -> Result<WritePermissions> {
    let mut out = WritePermissions::new();
    if value.is_undefined() {
        return Ok(out);
    }
    for entry in each(value, "permissions")? {
        let pair = each(&entry, "a permission entry")?;
        let [who, permission] = pair.as_slice() else {
            return Err(invalid("a permission entry must be [author, permission]"));
        };
        let who = author(&byte_value(who, "an author")?)?;
        let permission = match permission.as_string().as_deref() {
            Some("unbounded") => WritePermission::Unbounded,
            Some("denied") => WritePermission::Denied,
            Some(_) => {
                return Err(invalid(
                    "permission must be 'unbounded', 'denied', or hashes",
                ));
            }
            None => WritePermission::Bounded(hash_list(permission, "a write frontier")?),
        };
        out.insert(who, permission);
    }
    Ok(out)
}

fn permissions_js(permissions: &WritePermissions) -> Array {
    let mut entries: Vec<_> = permissions.iter().collect();
    entries.sort_by_key(|(a, _)| **a);
    entries
        .into_iter()
        .map(|(a, p)| {
            let value = match p {
                WritePermission::Unbounded => JsValue::from("unbounded"),
                WritePermission::Denied => JsValue::from("denied"),
                WritePermission::Bounded(frontier) => hash_array(frontier.iter().copied()).into(),
            };
            JsValue::from(Array::of2(&bytes(a.as_slice()), &value))
        })
        .collect()
}

fn format_id(format: &str) -> Result<FormatId> {
    core(FormatId::new(format))
}

fn limits(value: &JsValue) -> Result<Limits> {
    let mut limits = Limits::default();
    if value.is_undefined() {
        return Ok(limits);
    }
    let field = |name: &str| -> Result<Option<usize>> {
        let v = get(value, name);
        if v.is_undefined() {
            Ok(None)
        } else {
            number(&v, name).map(Some)
        }
    };
    if let Some(n) = field("maxHeaderBytes")? {
        limits.max_header_bytes = n;
    }
    if let Some(n) = field("maxMembers")? {
        limits.max_members = n;
    }
    if let Some(n) = field("maxParents")? {
        limits.max_parents = n;
    }
    if let Some(n) = field("maxSignatures")? {
        limits.max_signatures = n;
    }
    if let Some(n) = field("maxBoundaries")? {
        limits.max_boundaries = n;
    }
    if let Some(n) = field("maxCanonicalBytes")? {
        limits.max_canonical_bytes = n as u64;
    }
    if let Some(n) = field("maxItemBytes")? {
        limits.max_item_bytes = n as u64;
    }
    Ok(limits)
}

// ---- free functions ----

/// BLAKE3 of `data`: the content address of a stored blob.
#[wasm_bindgen(js_name = blobHash)]
pub fn blob_hash_of(data: &[u8]) -> Uint8Array {
    Uint8Array::from(BlobHash::of_contents(data).as_slice())
}

/// Parse a stored item's header from its first bytes. Returns `undefined`
/// until `prefix` holds the whole header, then `{ length, meta }`, where
/// `length` is where the encoded body starts.
#[wasm_bindgen(js_name = readHeader)]
pub fn read_header(prefix: &[u8], format: &str, limits_js: JsValue) -> Result<JsValue> {
    let limits = limits(&limits_js)?;
    let Some(length) = core(stockfoil_core::wire::header_len(prefix, &limits))? else {
        return Ok(JsValue::UNDEFINED);
    };
    if prefix.len() < length {
        return Ok(JsValue::UNDEFINED);
    }
    let decoded = core(stockfoil_core::wire::decode_header(
        &prefix[..length],
        &format_id(format)?,
        &limits,
    ))?;
    let o = Object::new();
    set(&o, "length", length as f64);
    set(&o, "meta", meta(&decoded));
    Ok(o.into())
}

// ---- History ----

/// Authenticated commit history. See the core `History` docs.
#[wasm_bindgen(js_name = History)]
pub struct JsHistory {
    inner: stockfoil_core::History,
    waiters: Vec<Function>,
}

impl Default for JsHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl JsHistory {
    /// Resolve every `epochChanged` promise if the epoch moved.
    fn notify(&mut self, before: u64) {
        if self.inner.epoch() != before {
            for resolve in std::mem::take(&mut self.waiters) {
                let _ = resolve.call0(&JsValue::UNDEFINED);
            }
        }
    }
}

#[wasm_bindgen(js_class = History)]
impl JsHistory {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: stockfoil_core::History::new(),
            waiters: Vec::new(),
        }
    }

    /// Number of delivered commits, drafts included.
    #[wasm_bindgen(getter)]
    pub fn length(&self) -> usize {
        self.inner.len()
    }

    /// Increments whenever anything is delivered or admitted.
    pub fn epoch(&self) -> u64 {
        self.inner.epoch()
    }

    /// Resolves once `epoch()` differs from `since` (immediately if it already does).
    #[wasm_bindgen(js_name = epochChanged)]
    pub fn epoch_changed(&mut self, since: u64) -> Promise {
        if self.inner.epoch() != since {
            return Promise::resolve(&JsValue::UNDEFINED);
        }
        let mut waiter = None;
        let promise = Promise::new(&mut |resolve, _| waiter = Some(resolve));
        self.waiters.extend(waiter);
        promise
    }

    /// `{ payload, author, admitted }`, or `undefined` if not delivered.
    pub fn lookup(&self, hash: &[u8]) -> Result<JsValue> {
        Ok(match self.inner.lookup(&commit_hash(hash)?) {
            None => JsValue::UNDEFINED,
            Some(info) => {
                let o = Object::new();
                set(&o, "payload", info.payload);
                set(&o, "author", bytes(info.author.as_slice()));
                set(&o, "admitted", info.admitted);
                o.into()
            }
        })
    }

    #[wasm_bindgen(js_name = isAdmitted)]
    pub fn is_admitted(&self, hash: &[u8]) -> Result<bool> {
        Ok(self.inner.is_admitted(&commit_hash(hash)?))
    }

    /// `"delivered"` (drafts included) or `"admitted"` heads, in hash order.
    pub fn heads(&self, which: &str) -> Result<Array> {
        let which = match which {
            "delivered" => Heads::Delivered,
            "admitted" => Heads::Admitted,
            _ => return Err(invalid("heads must be 'delivered' or 'admitted'")),
        };
        Ok(hash_array(self.inner.heads(which)))
    }

    /// Unadmitted local commits, parents first.
    pub fn drafts(&self) -> Array {
        hash_array(self.inner.drafts())
    }

    /// Ordered parents with repeats, or `undefined` if not delivered.
    pub fn parents(&self, hash: &[u8]) -> Result<JsValue> {
        Ok(self
            .inner
            .parents(&commit_hash(hash)?)
            .map_or(JsValue::UNDEFINED, |p| hash_array(p).into()))
    }

    /// Whether `ancestor` is a strict ancestor of `descendant`.
    #[wasm_bindgen(js_name = isAncestor)]
    pub fn is_ancestor(&self, ancestor: &[u8], descendant: &[u8]) -> Result<bool> {
        Ok(self
            .inner
            .is_ancestor(&commit_hash(ancestor)?, &commit_hash(descendant)?))
    }

    /// Start checking canonical bytes read back from storage against a
    /// delivered commit.
    #[wasm_bindgen(js_name = checkPayload)]
    pub fn check_payload(&self, hash: &[u8]) -> Result<PayloadCheck> {
        core(self.inner.check_payload(&commit_hash(hash)?)).map(PayloadCheck)
    }

    /// Start a local commit; every parent must be delivered.
    #[wasm_bindgen(js_name = beginCommit)]
    pub fn begin_commit(&self, author_bytes: &[u8], parents: JsValue) -> Result<CommitBuilder> {
        let parents = hash_list(&parents, "parents")?;
        core(self.inner.begin_commit(author(author_bytes)?, parents)).map(CommitBuilder)
    }

    /// Deliver an unsigned local draft. Returns `{ hash, payload, newlyDelivered }`.
    #[wasm_bindgen(js_name = publishCommit)]
    pub fn publish_commit(&mut self, prepared: &Prepared, payload: JsValue) -> Result<JsValue> {
        let before = self.inner.epoch();
        let result = core(
            self.inner
                .publish_commit(prepared.0.clone(), payload_id(&payload)?),
        );
        self.notify(before);
        Ok(delivered(&result?))
    }

    /// Attach a signature to a draft; returns the item IDs this admitted.
    pub fn sign(&mut self, author_bytes: &[u8], hash: &[u8], signature: &[u8]) -> Result<Array> {
        let signature = Signature::from(fixed::<64>(signature, "a signature")?);
        let before = self.inner.epoch();
        let result = core(
            self.inner
                .sign(author(author_bytes)?, commit_hash(hash)?, signature),
        );
        self.notify(before);
        Ok(item_ids(&result?))
    }

    /// Start admitting one stored item; the host answering its requests
    /// knows which. `options`: `{ format, readSize?, decodeCapacity?, quantum?,
    /// limits? }`.
    pub fn receive(&self, options: JsValue) -> Result<Admission> {
        Ok(Admission(self.inner.receive(receive_options(&options)?)))
    }

    /// Start decoding and checking wanted members (`Iterable<[index, commitHash]>`)
    /// from one stored item. `options`: `{ format, readSize?, decodeCapacity?, limits? }`.
    #[wasm_bindgen(js_name = decodeItem)]
    pub fn decode_item(&self, wanted: JsValue, options: JsValue) -> Result<DecodeItem> {
        let mut pairs = Vec::new();
        for entry in each(&wanted, "wanted")? {
            let pair = each(&entry, "a wanted member entry")?;
            let [index, hash] = pair.as_slice() else {
                return Err(invalid("a wanted member entry must be [index, commitHash]"));
            };
            pairs.push((
                number(index, "member index")?,
                commit_hash(&byte_value(hash, "a commit hash")?)?,
            ));
        }
        Ok(DecodeItem(
            core(self.inner.decode_item(receive_options(&options)?, pairs))?,
        ))
    }

    /// Dependency checks, placement, and atomic publication. Returns a `Next`.
    pub fn advance(&mut self, admission: &mut Admission) -> JsValue {
        let before = self.inner.epoch();
        let result = next(self.inner.advance(&mut admission.0));
        self.notify(before);
        result
    }

    /// Plan what a destination with `cursor` is missing.
    pub fn export(&self, cursor: &ExportCursor) -> ExportPlan {
        ExportPlan(self.inner.export(&cursor.0))
    }

    /// `{ id, header, members: [{ hash, payload }], meta }` for planned item
    /// `index`. `header` is the encoded wire header for `format`; append the
    /// codec's encoded body to it to form the item.
    #[wasm_bindgen(js_name = exportItem)]
    pub fn export_item(&self, plan: &ExportPlan, index: usize, format: &str) -> Result<JsValue> {
        let item = core(self.inner.export_item(&plan.0, index))?;
        let header = stockfoil_core::wire::encode_header(&item.meta, &format_id(format)?);
        let o = Object::new();
        set(&o, "id", item_id(&item.meta.id));
        set(&o, "header", bytes(&header));
        let members: Array = item
            .members
            .iter()
            .map(|(hash, payload)| {
                let m = Object::new();
                set(&m, "hash", bytes(hash.as_slice()));
                set(&m, "payload", *payload);
                JsValue::from(m)
            })
            .collect();
        set(&o, "members", members);
        set(&o, "meta", meta(&item.meta));
        Ok(o.into())
    }

    /// The working view (drafts included). `permissions` is an iterable of
    /// `[author, "unbounded" | "denied" | frontierHashes]`, e.g. a `Map`.
    #[wasm_bindgen(js_name = viewPoint)]
    pub fn view_point(&self, permissions_js: JsValue) -> Result<ViewPoint> {
        Ok(ViewPoint(
            self.inner.view_point(&permissions(&permissions_js)?),
        ))
    }

    /// The authenticated view.
    #[wasm_bindgen(js_name = signedViewPoint)]
    pub fn signed_view_point(&self, permissions_js: JsValue) -> Result<ViewPoint> {
        Ok(ViewPoint(
            self.inner.signed_view_point(&permissions(&permissions_js)?),
        ))
    }

    /// Start a change traversal from a materializer's cursor to `target`.
    pub fn changes(&self, cursor: &MaterializationCursor, target: &ViewPoint) -> Result<Changes> {
        core(self.inner.changes(&cursor.0, &target.0)).map(Changes)
    }

    /// Up to `max` records: `{ records, complete, cursor }`. `cursor` is set
    /// only once `complete`.
    #[wasm_bindgen(js_name = nextChanges)]
    pub fn next_changes(&self, changes: &mut Changes, max: usize) -> Result<JsValue> {
        let mut out = Vec::new();
        let batch = core(self.inner.next_changes(&mut changes.0, &mut out, max))?;
        let o = Object::new();
        set(&o, "records", out.iter().map(change).collect::<Array>());
        match batch {
            stockfoil_core::Batch::More => {
                set(&o, "complete", false);
                set(&o, "cursor", JsValue::UNDEFINED);
            }
            stockfoil_core::Batch::Complete(cursor) => {
                set(&o, "complete", true);
                set(&o, "cursor", MaterializationCursor(cursor));
            }
        }
        Ok(o.into())
    }
}

fn change(c: &Change) -> JsValue {
    let o = Object::new();
    set(&o, "hash", bytes(c.hash.as_slice()));
    set(&o, "payload", c.payload);
    set(&o, "newlyDelivered", c.newly_delivered);
    set(
        &o,
        "visibility",
        match c.visibility {
            Vis::Before => "before",
            Vis::After => "after",
            Vis::Both => "both",
            Vis::Neither => "neither",
        },
    );
    o.into()
}

// ---- local commits ----

#[wasm_bindgen]
pub struct CommitBuilder(stockfoil_core::CommitBuilder);

#[wasm_bindgen]
impl CommitBuilder {
    /// Append canonical payload bytes.
    pub fn update(&mut self, canonical: &[u8]) {
        self.0.update(canonical);
    }
    /// Consumes the builder.
    pub fn finish(self) -> Prepared {
        Prepared(self.0.finish())
    }
}

/// Checks one payload read back from storage.
#[wasm_bindgen]
pub struct PayloadCheck(stockfoil_core::PayloadCheck);

#[wasm_bindgen]
impl PayloadCheck {
    /// Append canonical payload bytes.
    pub fn update(&mut self, canonical: &[u8]) {
        self.0.update(canonical);
    }
    /// Consumes the check. Throws `CorruptPayload` unless the bytes are the
    /// commit's payload.
    pub fn finish(self) -> Result<()> {
        core(self.0.finish())
    }
}

#[wasm_bindgen]
pub struct Prepared(stockfoil_core::Prepared);

#[wasm_bindgen]
impl Prepared {
    #[wasm_bindgen(getter)]
    pub fn hash(&self) -> Uint8Array {
        Uint8Array::from(self.0.hash.as_slice())
    }
    /// The 32 bytes the author signs with Ed25519 (the raw commit hash).
    #[wasm_bindgen(getter, js_name = signingBytes)]
    pub fn signing_bytes(&self) -> Uint8Array {
        Uint8Array::from(self.0.signing_bytes().as_slice())
    }
    #[wasm_bindgen(getter)]
    pub fn author(&self) -> Uint8Array {
        Uint8Array::from(self.0.author.as_slice())
    }
    #[wasm_bindgen(getter)]
    pub fn parents(&self) -> Array {
        hash_array(self.0.parents.iter().copied())
    }
    #[wasm_bindgen(getter, js_name = payloadHash)]
    pub fn payload_hash(&self) -> Uint8Array {
        Uint8Array::from(self.0.payload_hash.as_slice())
    }
    /// Canonical payload length in bytes.
    #[wasm_bindgen(getter)]
    pub fn length(&self) -> f64 {
        self.0.len as f64
    }
}

// ---- admission ----

/// Receiving one stored item through host requests.
#[wasm_bindgen]
pub struct Admission(stockfoil_core::Admission);

#[wasm_bindgen]
impl Admission {
    /// History-free work and the next host step. Returns an owned `Next`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> JsValue {
        next(self.0.next())
    }

    /// Answer the outstanding request. Replies are copied in. Throws a
    /// `StockfoilError` with code `StaleReply` or `WrongKindReply` (the
    /// operation is unaffected), or with the core error if the reply broke the
    /// protocol (the operation has then failed).
    pub fn reply(&mut self, ticket: u64, reply: JsValue) -> Result<()> {
        accept_reply(ticket, reply, |t, r| self.0.reply(t, r))
    }

    /// `{ ticket, request }` for the outstanding request, or `undefined`.
    pub fn request(&self) -> JsValue {
        self.0
            .request()
            .map_or(JsValue::UNDEFINED, |(t, r)| request(t, r))
    }

    /// `{ bytesRead, encodedBytesDecoded, canonicalBytes, requests }`.
    pub fn counts(&self) -> JsValue {
        counts_js(self.0.counts())
    }

    /// The parsed (untrusted) header metadata once read, else `undefined`.
    pub fn meta(&self) -> JsValue {
        self.0.meta().map_or(JsValue::UNDEFINED, |m| meta(m))
    }
}

/// Decoding and checking wanted members of one stored item through host requests.
#[wasm_bindgen]
pub struct DecodeItem(stockfoil_core::DecodeItem);

#[wasm_bindgen]
impl DecodeItem {
    /// Advance the operation and return an owned `DecodeNext`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> JsValue {
        decode_next(self.0.next())
    }

    /// Answer the outstanding request.
    pub fn reply(&mut self, ticket: u64, reply: JsValue) -> Result<()> {
        accept_reply(ticket, reply, |t, r| self.0.reply(t, r))
    }

    /// `{ ticket, request }` for the outstanding request, or `undefined`.
    pub fn request(&self) -> JsValue {
        self.0
            .request()
            .map_or(JsValue::UNDEFINED, |(t, r)| request(t, r))
    }

    /// `{ bytesRead, encodedBytesDecoded, canonicalBytes, requests }`.
    pub fn counts(&self) -> JsValue {
        counts_js(self.0.counts())
    }

    /// The parsed header metadata once read, else `undefined`.
    pub fn meta(&self) -> JsValue {
        self.0.meta().map_or(JsValue::UNDEFINED, |m| meta(m))
    }
}

// ---- export ----

#[wasm_bindgen]
pub struct ExportPlan(stockfoil_core::ExportPlan);

#[wasm_bindgen]
impl ExportPlan {
    /// Number of items to send.
    #[wasm_bindgen(getter)]
    pub fn length(&self) -> usize {
        self.0.len()
    }
    pub fn id(&self, index: usize) -> JsValue {
        self.0.id(index).map_or(JsValue::UNDEFINED, item_id)
    }
    /// Items the destination holds that a fragment now covers.
    pub fn retired(&self) -> Array {
        item_ids(self.0.retired())
    }
    /// Record that the destination durably accepted item `index`.
    pub fn acknowledge(&mut self, index: usize) -> Result<()> {
        self.0.acknowledge(index).map_err(|e| core_error(&e))
    }
    /// The cursor after the acknowledged items.
    pub fn cursor(&self) -> ExportCursor {
        ExportCursor(self.0.cursor())
    }
}

#[wasm_bindgen]
pub struct ExportCursor(stockfoil_core::ExportCursor);

#[wasm_bindgen]
impl ExportCursor {
    #[wasm_bindgen(constructor)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(stockfoil_core::ExportCursor::new())
    }
    #[wasm_bindgen(getter)]
    pub fn length(&self) -> usize {
        self.0.len()
    }
    pub fn items(&self) -> Array {
        item_ids(self.0.items())
    }
    #[wasm_bindgen(js_name = toBytes)]
    pub fn to_bytes(&self) -> Uint8Array {
        Uint8Array::from(self.0.to_bytes().as_slice())
    }
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(data: &[u8]) -> Result<ExportCursor> {
        core(stockfoil_core::ExportCursor::from_bytes(data)).map(Self)
    }
}

// ---- views and changes ----

#[wasm_bindgen]
pub struct ViewPoint(stockfoil_core::ViewPoint);

#[wasm_bindgen]
impl ViewPoint {
    /// A view from explicit heads and permissions (see `History.viewPoint`).
    #[wasm_bindgen(constructor)]
    pub fn new(heads: JsValue, permissions_js: JsValue) -> Result<ViewPoint> {
        Ok(Self(stockfoil_core::ViewPoint::from_parts(
            hash_list(&heads, "heads")?,
            permissions(&permissions_js)?,
        )))
    }
    pub fn heads(&self) -> Array {
        hash_array(self.0.heads().iter().copied())
    }
    /// `[author, permission][]`, sorted by author.
    pub fn permissions(&self) -> Array {
        permissions_js(self.0.write_permissions())
    }
}

#[wasm_bindgen]
pub struct MaterializationCursor(stockfoil_core::MaterializationCursor);

#[wasm_bindgen]
impl MaterializationCursor {
    #[wasm_bindgen(constructor)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(stockfoil_core::MaterializationCursor::new())
    }
    #[wasm_bindgen(js_name = deliveryHeads)]
    pub fn delivery_heads(&self) -> Array {
        hash_array(self.0.delivery_heads().iter().copied())
    }
    #[wasm_bindgen(js_name = viewPoint)]
    pub fn view_point(&self) -> ViewPoint {
        ViewPoint(self.0.view_point().clone())
    }
    #[wasm_bindgen(js_name = toBytes)]
    pub fn to_bytes(&self) -> Uint8Array {
        Uint8Array::from(self.0.to_bytes().as_slice())
    }
    #[wasm_bindgen(js_name = fromBytes)]
    pub fn from_bytes(data: &[u8]) -> Result<MaterializationCursor> {
        core(stockfoil_core::MaterializationCursor::from_bytes(data)).map(Self)
    }
}

/// A captured change traversal.
#[wasm_bindgen]
pub struct Changes(stockfoil_core::Changes);

#[wasm_bindgen]
impl Changes {
    /// Records remaining in this traversal.
    #[wasm_bindgen(getter)]
    pub fn remaining(&self) -> usize {
        self.0.remaining()
    }
}
