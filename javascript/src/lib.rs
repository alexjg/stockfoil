//! Thin wasm-bindgen binding over the Stockfoil core.
//!
//! Every value crossing the boundary is owned: bytes are copied into fresh
//! `Uint8Array`s on the way out and into Rust buffers on the way in, so nothing
//! borrowed from Wasm memory survives a call. Hashes, authors, and signatures
//! are `Uint8Array`s (32/32/64 bytes); payload IDs and tickets are `BigInt`s.
//! The host loop, codecs, and storage live in the TypeScript wrapper.
use js_sys::{Array, Function, Object, Reflect, Uint8Array};
use stockfoil::{
    Author, BundleMeta, CommitHash, Counts, DecodeNext, DecodeStatus, Delivered, Error, FormatId,
    Heads, ItemId, Limits, Next, ParentRef, PayloadId, Receipt, ReceiveOptions, Reply, ReplyError,
    Request, Signature, Ticket, WritePermission, WritePermissions,
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

fn integer(value: &JsValue, what: &str) -> Result<u64> {
    value
        .as_f64()
        .filter(|n| n.fract() == 0.0 && *n >= 0.0 && *n <= 9007199254740991.0)
        .map(|n| n as u64)
        .ok_or_else(|| invalid(&format!("{what} must be a non-negative safe integer")))
}

fn number(value: &JsValue, what: &str) -> Result<usize> {
    usize::try_from(integer(value, what)?)
        .map_err(|_| invalid(&format!("{what} exceeds this engine's address space")))
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

/// `{ id, members: [{ author, parents: (number | Uint8Array)[] }] }`.
/// A numeric parent is an earlier member's index; bytes are an external hash.
fn meta(meta: &BundleMeta) -> JsValue {
    let o = Object::new();
    set(&o, "id", item_id(&meta.id));
    let members: Array = meta
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
    for (name, slot) in [
        ("maxCanonicalBytes", &mut limits.max_canonical_bytes),
        ("maxItemBytes", &mut limits.max_item_bytes),
    ] {
        let v = get(value, name);
        if !v.is_undefined() {
            *slot = integer(&v, name)?;
        }
    }
    Ok(limits)
}

// Stateful engines are private implementation details of the TS facade.
// All descriptions, events, cursors, drafts and checkpoints are owned data.

fn read_item(value: &JsValue) -> Result<ItemId> {
    let head = commit_hash(&byte_value(&get(value, "head"), "item.head")?)?;
    match get(value, "kind").as_string().as_deref() {
        Some("commit") => Ok(ItemId::Commit(head)),
        Some("fragment") => Ok(ItemId::Fragment(stockfoil::FragmentRange {
            head,
            boundary: hash_list(&get(value, "boundary"), "item.boundary")?
                .into_iter()
                .collect(),
        })),
        _ => Err(invalid("invalid item kind")),
    }
}

fn read_cursor(value: &JsValue) -> Result<stockfoil::ExportCursor> {
    Ok(stockfoil::ExportCursor::from_items(
        each(value, "cursor")?
            .iter()
            .map(read_item)
            .collect::<Result<Vec<_>>>()?,
    ))
}

#[wasm_bindgen(js_name = encodeExportCursor)]
pub fn encode_export_cursor(value: JsValue) -> Result<Vec<u8>> {
    Ok(read_cursor(&value)?.to_bytes())
}

#[wasm_bindgen(js_name = decodeExportCursor)]
pub fn decode_export_cursor(data: &[u8]) -> Result<Array> {
    Ok(item_ids(
        core(stockfoil::ExportCursor::from_bytes(data))?.items(),
    ))
}

fn follow(value: &JsValue) -> Result<stockfoil::Follow> {
    match value.as_string().as_deref() {
        Some("working") => Ok(stockfoil::Follow::Working),
        Some("signed") => Ok(stockfoil::Follow::Signed),
        _ if get(value, "kind").as_string().as_deref() == Some("pinned") => Ok(
            stockfoil::Follow::Pinned(hash_list(&get(value, "heads"), "follow.heads")?),
        ),
        _ => Err(invalid(
            "follow must be 'working', 'signed', or {kind: 'pinned', heads}",
        )),
    }
}

fn follow_js(value: &stockfoil::Follow) -> JsValue {
    match value {
        stockfoil::Follow::Working => "working".into(),
        stockfoil::Follow::Signed => "signed".into(),
        stockfoil::Follow::Pinned(heads) => {
            let o = Object::new();
            set(&o, "kind", "pinned");
            set(&o, "heads", hash_array(heads.iter().copied()));
            o.into()
        }
    }
}

fn draft(value: &stockfoil::Draft) -> JsValue {
    let o = Object::new();
    set(&o, "hash", bytes(value.hash.as_slice()));
    set(&o, "author", bytes(value.author.as_slice()));
    set(&o, "parents", hash_array(value.parents.iter().copied()));
    o.into()
}

fn read_draft(value: &JsValue) -> Result<stockfoil::Draft> {
    Ok(stockfoil::Draft {
        hash: commit_hash(&byte_value(&get(value, "hash"), "draft.hash")?)?,
        author: author(&byte_value(&get(value, "author"), "draft.author")?)?,
        parents: hash_list(&get(value, "parents"), "draft.parents")?,
    })
}

fn failures(value: &[(ItemId, Error)]) -> Array {
    value
        .iter()
        .map(|(id, error)| {
            let o = Object::new();
            set(&o, "id", item_id(id));
            set(&o, "error", core_error(error));
            JsValue::from(o)
        })
        .collect()
}

// Bind is deliberately synchronous: stage payloads during caller-owned I/O,
// then return their assigned IDs here. Never reenter the ledger from Bind.
fn binder(f: Function) -> impl FnMut(&ItemId, &[CommitHash]) -> Vec<PayloadId> {
    move |id, members| {
        f.call2(
            &JsValue::UNDEFINED,
            &item_id(id),
            &hash_array(members.iter().copied()),
        )
        .and_then(|v| each(&v, "bound IDs"))
        .and_then(|v| v.iter().map(payload_id).collect::<Result<Vec<_>>>())
        // An invalid host binding fails the admission's Bound protocol check.
        .unwrap_or_default()
    }
}

#[wasm_bindgen(js_name = LedgerEngine)]
pub struct JsLedger(stockfoil::Ledger);

#[wasm_bindgen(js_class = LedgerEngine)]
impl JsLedger {
    #[wasm_bindgen(constructor)]
    pub fn new(options: JsValue) -> Result<Self> {
        let opts = receive_options(&options)?;
        let who = author(&byte_value(&get(&options, "author"), "options.author")?)?;
        let mut ledger = stockfoil::Ledger::new(opts.format, who).with_limits(opts.limits);
        let max = get(&options, "maxParked");
        if !max.is_undefined() {
            ledger = ledger.with_max_parked(number(&max, "maxParked")?);
        }
        let perms = get(&options, "permissions");
        if !perms.is_undefined() {
            ledger.set_permissions(permissions(&perms)?);
        }
        Ok(Self(ledger))
    }
    #[wasm_bindgen(getter)]
    pub fn length(&self) -> usize {
        self.0.len()
    }
    pub fn epoch(&self) -> u64 {
        self.0.epoch()
    }
    pub fn author(&self) -> Vec<u8> {
        self.0.author().as_slice().to_vec()
    }
    pub fn heads(&self, which: &str) -> Result<Array> {
        let which = match which {
            "delivered" => Heads::Delivered,
            "admitted" => Heads::Admitted,
            _ => return Err(invalid("heads must be 'delivered' or 'admitted'")),
        };
        Ok(hash_array(self.0.heads(which)))
    }
    pub fn lookup(&self, hash: &[u8]) -> Result<JsValue> {
        Ok(match self.0.history().lookup(&commit_hash(hash)?) {
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
    pub fn parents(&self, hash: &[u8]) -> Result<JsValue> {
        Ok(self
            .0
            .parents(&commit_hash(hash)?)
            .map_or(JsValue::UNDEFINED, |p| hash_array(p).into()))
    }
    #[wasm_bindgen(js_name = isAncestor)]
    pub fn is_ancestor(&self, a: &[u8], d: &[u8]) -> Result<bool> {
        Ok(self.0.is_ancestor(&commit_hash(a)?, &commit_hash(d)?))
    }
    pub fn permissions(&self) -> Array {
        permissions_js(self.0.permissions())
    }
    #[wasm_bindgen(js_name = setPermissions)]
    pub fn set_permissions(&mut self, value: JsValue) -> Result<()> {
        self.0.set_permissions(permissions(&value)?);
        Ok(())
    }
    pub fn view(&self, rule: JsValue) -> Result<JsValue> {
        let view = self.0.view(&follow(&rule)?);
        let o = Object::new();
        set(&o, "heads", hash_array(view.heads().iter().copied()));
        set(&o, "permissions", permissions_js(view.write_permissions()));
        Ok(o.into())
    }
    #[wasm_bindgen(js_name = beginCommit)]
    pub fn begin_commit(&self, parents: JsValue) -> Result<CommitBuilder> {
        let parents = if parents.is_undefined() {
            None
        } else {
            Some(hash_list(&parents, "parents")?)
        };
        Ok(CommitBuilder(core(self.0.begin_commit(parents))?))
    }
    pub fn commit(&mut self, builder: CommitBuilder, payload: JsValue) -> Result<JsValue> {
        Ok(draft(&core(
            self.0.commit(builder.0.finish(), payload_id(&payload)?),
        )?))
    }
    pub fn drafts(&self) -> Array {
        self.0.drafts().map(|d| draft(&d)).collect()
    }
    #[wasm_bindgen(js_name = attachSignature)]
    pub fn attach_signature(
        &mut self,
        value: JsValue,
        signature: &[u8],
        bind: Function,
    ) -> Result<JsValue> {
        let signed = core(self.0.attach_signature(
            &read_draft(&value)?,
            Signature::from(fixed::<64>(signature, "a signature")?),
            &mut binder(bind),
        ))?;
        let o = Object::new();
        set(&o, "admitted", hash_array(signed.admitted));
        set(
            &o,
            "released",
            signed.released.iter().map(receipt).collect::<Array>(),
        );
        set(&o, "failed", failures(&signed.failed));
        Ok(o.into())
    }
    #[wasm_bindgen(js_name = beginReceive)]
    pub fn begin_receive(&self, options: JsValue) -> Result<Admission> {
        Ok(Admission(core(
            self.0.begin_receive_with(receive_options(&options)?),
        )?))
    }
    pub fn admit(&mut self, op: Admission, bind: Function) -> Result<JsValue> {
        let result = core(self.0.admit(op.0, &mut binder(bind)))?;
        let o = Object::new();
        match result {
            stockfoil::Received::Admitted {
                items,
                delivered,
                failed,
            } => {
                set(&o, "kind", "admitted");
                set(&o, "items", items.iter().map(receipt).collect::<Array>());
                set(&o, "delivered", hash_array(delivered));
                set(&o, "failed", failures(&failed));
            }
            stockfoil::Received::Waiting(hash) => {
                set(&o, "kind", "waiting");
                set(&o, "waiting", bytes(hash.as_slice()));
            }
            stockfoil::Received::Evicted { waiting, evicted } => {
                set(&o, "kind", "evicted");
                set(&o, "waiting", bytes(waiting.as_slice()));
                set(&o, "evicted", item_id(&evicted));
            }
        }
        Ok(o.into())
    }
    pub fn parked(&self) -> usize {
        self.0.parked()
    }
    pub fn missing(&self) -> Array {
        hash_array(self.0.missing())
    }
    pub fn export(&self, cursor: JsValue) -> Result<JsValue> {
        let plan = self.0.export(&read_cursor(&cursor)?);
        let items = Array::new();
        for index in 0..plan.len() {
            let item = core(self.0.export_item(&plan, index))?;
            let o = Object::new();
            set(&o, "id", item_id(&item.id));
            set(&o, "header", bytes(&item.header));
            set(&o, "meta", meta(&item.meta));
            set(&o, "checkpoints", hash_array(item.checkpoints));
            set(
                &o,
                "members",
                item.members
                    .iter()
                    .map(|(hash, id)| {
                        let m = Object::new();
                        set(&m, "hash", bytes(hash.as_slice()));
                        set(&m, "payload", *id);
                        JsValue::from(m)
                    })
                    .collect::<Array>(),
            );
            items.push(&o);
        }
        let o = Object::new();
        set(&o, "items", items);
        set(&o, "retired", item_ids(plan.retired()));
        set(&o, "cursor", item_ids(plan.cursor().items()));
        Ok(o.into())
    }
    #[wasm_bindgen(js_name = checkPayload)]
    pub fn check_payload(&self, hash: &[u8]) -> Result<PayloadCheck> {
        core(self.0.history().check_payload(&commit_hash(hash)?)).map(PayloadCheck)
    }
    #[wasm_bindgen(js_name = decodeItem)]
    pub fn decode_item(&self, wanted: JsValue, options: JsValue) -> Result<DecodeItem> {
        let mut pairs = Vec::new();
        for entry in each(&wanted, "wanted")? {
            let pair = each(&entry, "wanted member")?;
            let [index, hash] = pair.as_slice() else {
                return Err(invalid("expected [index, hash]"));
            };
            pairs.push((
                number(index, "index")?,
                commit_hash(&byte_value(hash, "hash")?)?,
            ));
        }
        Ok(DecodeItem(core(
            self.0
                .history()
                .decode_item(receive_options(&options)?, pairs),
        )?))
    }
    pub fn ancestry(&self, heads: JsValue) -> Result<Ancestry> {
        Ok(Ancestry(core(
            self.0.ancestry(&hash_list(&heads, "heads")?),
        )?))
    }
}

#[wasm_bindgen]
pub struct CommitBuilder(stockfoil::CommitBuilder);
#[wasm_bindgen]
impl CommitBuilder {
    pub fn update(&mut self, canonical: &[u8]) {
        self.0.update(canonical);
    }
    /// Inspect the hash without consuming the streaming builder.
    pub fn hash(&self) -> Vec<u8> {
        self.0.clone().finish().hash.as_slice().to_vec()
    }
}

/// Checks one payload read back from storage.
#[wasm_bindgen]
pub struct PayloadCheck(stockfoil::PayloadCheck);

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

// ---- admission ----

/// Receiving one stored item through host requests.
#[wasm_bindgen]
pub struct Admission(stockfoil::Admission);

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
pub struct DecodeItem(stockfoil::DecodeItem);

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

#[wasm_bindgen]
pub struct Ancestry(stockfoil::Ancestry);
#[wasm_bindgen]
impl Ancestry {
    pub fn contains(&self, ledger: &JsLedger, hash: &[u8]) -> Result<bool> {
        core(self.0.contains(ledger.0.history(), &commit_hash(hash)?))
    }
    pub fn participates(&self, ledger: &JsLedger, hash: &[u8]) -> Result<bool> {
        core(self.0.participates(ledger.0.history(), &commit_hash(hash)?))
    }
    pub fn difference(&self, other: &Ancestry, ledger: &JsLedger) -> Result<Array> {
        Ok(hash_array(core(
            self.0.difference(&other.0, ledger.0.history()),
        )?))
    }
}

fn event(value: &stockfoil::Event<PayloadId>) -> JsValue {
    use stockfoil::Event::*;
    let o = Object::new();
    match value {
        Reset => set(&o, "kind", "reset"),
        Deliver {
            hash,
            author,
            parents,
            payload,
            active,
        } => {
            set(&o, "kind", "deliver");
            set(&o, "hash", bytes(hash.as_slice()));
            set(&o, "author", bytes(author.as_slice()));
            set(&o, "parents", hash_array(parents.iter().copied()));
            set(&o, "payload", *payload);
            set(&o, "active", *active);
        }
        Activate { hash } | Retract { hash } => {
            set(
                &o,
                "kind",
                if matches!(value, Activate { .. }) {
                    "activate"
                } else {
                    "retract"
                },
            );
            set(&o, "hash", bytes(hash.as_slice()));
        }
        Payload { hash, payload } => {
            set(&o, "kind", "payload");
            set(&o, "hash", bytes(hash.as_slice()));
            set(&o, "payload", *payload);
        }
    }
    o.into()
}

fn read_event(v: &JsValue) -> Result<stockfoil::Event<PayloadId>> {
    use stockfoil::Event::*;
    let kind = get(v, "kind").as_string().unwrap_or_default();
    if kind == "reset" {
        return Ok(Reset);
    }
    let hash = commit_hash(&byte_value(&get(v, "hash"), "event.hash")?)?;
    Ok(match kind.as_str() {
        "deliver" => Deliver {
            hash,
            author: author(&byte_value(&get(v, "author"), "event.author")?)?,
            parents: hash_list(&get(v, "parents"), "event.parents")?,
            payload: payload_id(&get(v, "payload"))?,
            active: get(v, "active")
                .as_bool()
                .ok_or_else(|| invalid("event.active must be boolean"))?,
        },
        "activate" => Activate { hash },
        "retract" => Retract { hash },
        "payload" => Payload {
            hash,
            payload: payload_id(&get(v, "payload"))?,
        },
        _ => return Err(invalid("invalid event kind")),
    })
}

fn applied(v: &JsValue) -> Result<stockfoil::Applied> {
    let mut out = stockfoil::Applied::default();
    let rejected = get(v, "rejected");
    if !rejected.is_undefined() {
        for r in each(&rejected, "rejected")? {
            out.rejected.push(stockfoil::Rejection {
                hash: commit_hash(&byte_value(&get(&r, "hash"), "rejection.hash")?)?,
                reason: get(&r, "reason").as_string().unwrap_or_default(),
                cascade: get(&r, "cascade").as_bool().unwrap_or(false),
            });
        }
    }
    let wanted = get(v, "wanted");
    if !wanted.is_undefined() {
        out.wanted = hash_list(&wanted, "wanted")?;
    }
    Ok(out)
}

static NEXT_BATCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[wasm_bindgen(js_name = MaterializationEngine)]
pub struct JsMaterialization {
    inner: stockfoil::Materialization,
    pending: Option<(u64, stockfoil::Batch)>,
}
impl JsMaterialization {
    fn install(&mut self, batch: Option<stockfoil::Batch>) -> JsValue {
        match batch {
            None => JsValue::UNDEFINED,
            Some(batch) => {
                let token = NEXT_BATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let o = Object::new();
                set(&o, "token", token);
                set(
                    &o,
                    "events",
                    batch.events().iter().map(event).collect::<Array>(),
                );
                set(&o, "rebuild", batch.is_rebuild());
                self.pending = Some((token, batch));
                o.into()
            }
        }
    }
}
#[wasm_bindgen(js_class = MaterializationEngine)]
impl JsMaterialization {
    #[wasm_bindgen(constructor)]
    pub fn new(rule: JsValue, visibility: bool, checkpoint: Option<Vec<u8>>) -> Result<Self> {
        let cp = match checkpoint {
            Some(b) => core(stockfoil::Checkpoint::from_bytes(&b))?,
            None => stockfoil::Checkpoint::new(follow(&rule)?),
        };
        Ok(Self {
            inner: stockfoil::Materialization::resume(cp, visibility),
            pending: None,
        })
    }
    pub fn checkpoint(&self) -> Vec<u8> {
        self.inner.checkpoint().to_bytes()
    }
    pub fn follow(&self) -> JsValue {
        follow_js(self.inner.follow())
    }
    #[wasm_bindgen(js_name = setFollow)]
    pub fn set_follow(&mut self, rule: JsValue) -> Result<()> {
        self.inner.set_follow(follow(&rule)?);
        Ok(())
    }
    pub fn heads(&self) -> Array {
        hash_array(self.inner.heads().iter().copied())
    }
    #[wasm_bindgen(js_name = isCurrent)]
    pub fn is_current(&self, ledger: &JsLedger) -> bool {
        self.inner.is_current(&ledger.0)
    }
    pub fn plan(&mut self, ledger: &JsLedger, budget: usize) -> Result<JsValue> {
        let batch = core(self.inner.plan(&ledger.0, budget))?;
        Ok(self.install(batch))
    }
    pub fn complete(
        &mut self,
        ledger: &JsLedger,
        value: JsValue,
        result: JsValue,
    ) -> Result<JsValue> {
        let (token, batch) = self
            .pending
            .as_ref()
            .ok_or_else(|| core_error(&Error::StaleBatch))?;
        if payload_id(&get(&value, "token"))? != *token
            || get(&value, "rebuild").as_bool() != Some(batch.is_rebuild())
            || each(&get(&value, "events"), "events")?
                .iter()
                .map(read_event)
                .collect::<Result<Vec<_>>>()?
                != batch.events()
        {
            return Err(core_error(&Error::StaleBatch));
        }
        let result = applied(&result)?;
        // Preserve the token on all core preflight errors, including wrong
        // ledger, invalid rejections and unknown wanted commits.
        let copy = batch.clone();
        let next = core(self.inner.complete(&ledger.0, copy, result))?;
        self.pending = None;
        Ok(self.install(next))
    }
    #[wasm_bindgen(js_name = markLocal)]
    pub fn mark_local(&mut self, ledger: &JsLedger, value: JsValue) -> Result<()> {
        core(self.inner.mark_local(&ledger.0, &read_draft(&value)?))
    }
}

#[wasm_bindgen(js_name = inspectCheckpoint)]
pub fn inspect_checkpoint(data: &[u8]) -> Result<JsValue> {
    let cp = core(stockfoil::Checkpoint::from_bytes(data))?;
    let o = Object::new();
    set(&o, "follow", follow_js(cp.follow()));
    set(&o, "heads", hash_array(cp.heads().iter().copied()));
    set(&o, "partial", cp.is_partial());
    set(
        &o,
        "rejected",
        cp.rejected()
            .map(|(hash, cascade)| {
                let r = Object::new();
                set(&r, "hash", bytes(hash.as_slice()));
                set(&r, "cascade", cascade);
                JsValue::from(r)
            })
            .collect::<Array>(),
    );
    Ok(o.into())
}
