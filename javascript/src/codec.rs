//! The JS payload codec adapter and handle for the single native graph.
use crate::{
    RawExportCursor, RawMaterializationCursor, decode, dto, encode, error::Error, require_map, set,
};
use js_sys::{Array, Function, Uint8Array};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    sync::{Mutex, MutexGuard, TryLockError},
};
use stockfoil::{DecodeLimits, FragmentShape, ParentRef, PayloadCodec, Vis};
use wasm_bindgen::{JsCast, prelude::*};

#[derive(Debug)]
struct CodecError(JsValue);
impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "application payload codec failed")
    }
}
impl std::error::Error for CodecError {}
impl From<JsValue> for CodecError {
    fn from(value: JsValue) -> Self {
        Self(value)
    }
}

struct JsCodec {
    bridge: JsValue,
    format: String,
}
impl JsCodec {
    fn invoke(&self, name: &str, args: &[JsValue]) -> Result<JsValue, CodecError> {
        let function = js_sys::Reflect::get(&self.bridge, &name.into())?
            .dyn_into::<Function>()
            .map_err(|_| CodecError(Error::input("missing codec callback").into()))?;
        let array = Array::new();
        for arg in args {
            array.push(arg);
        }
        Ok(function.apply(&self.bridge, &array)?)
    }
    fn copy(&self, value: &JsValue) -> Result<JsValue, JsValue> {
        self.invoke("clonePayload", &[value.clone()])
            .map_err(|e| codec_error(e.0))
    }
    fn bytes(&self, name: &str, args: &[JsValue]) -> Result<Vec<u8>, CodecError> {
        let value = self.invoke(name, args)?;
        let bytes = value
            .dyn_into::<Uint8Array>()
            .map_err(|_| CodecError(Error::input("codec encoder must return Uint8Array").into()))?;
        Ok(bytes.to_vec())
    }
}
fn codec_error(cause: JsValue) -> JsValue {
    let error: JsValue = Error::new("CodecError", "application payload codec failed").into();
    let _ = set(&error, "cause", &cause);
    error
}
fn core_error(error: stockfoil::Error<CodecError>) -> JsValue {
    let value: JsValue = Error::new(error.code(), &error).into();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        if let Some(cause) = cause.downcast_ref::<CodecError>() {
            let _ = set(&value, "cause", &cause.0);
            break;
        }
        source = cause.source();
    }
    value
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Limits {
    max_item_bytes: usize,
    max_members: usize,
    max_parents: usize,
    max_signatures: usize,
    max_payload_bytes: usize,
}
impl From<&DecodeLimits> for Limits {
    fn from(l: &DecodeLimits) -> Self {
        Self {
            max_item_bytes: l.max_item_bytes,
            max_members: l.max_members,
            max_parents: l.max_parents,
            max_signatures: l.max_signatures,
            max_payload_bytes: l.max_payload_bytes,
        }
    }
}
impl From<Limits> for DecodeLimits {
    fn from(l: Limits) -> Self {
        Self {
            max_item_bytes: l.max_item_bytes,
            max_members: l.max_members,
            max_parents: l.max_parents,
            max_signatures: l.max_signatures,
            max_payload_bytes: l.max_payload_bytes,
        }
    }
}
fn shape(value: &FragmentShape) -> Result<JsValue, JsValue> {
    #[derive(Serialize)]
    struct Shape {
        members: Vec<Member>,
    }
    #[derive(Serialize)]
    struct Member {
        author: String,
        parents: Vec<Parent>,
    }
    #[derive(Serialize)]
    #[serde(tag = "kind", rename_all = "camelCase")]
    enum Parent {
        Member { index: usize },
        External { hash: String },
    }
    encode(&Shape {
        members: value
            .members
            .iter()
            .map(|m| Member {
                author: dto::hex(m.author.as_slice()),
                parents: m
                    .parents
                    .iter()
                    .map(|p| match p {
                        ParentRef::Member(index) => Parent::Member { index: *index },
                        ParentRef::External(hash) => Parent::External {
                            hash: dto::hex(hash.as_slice()),
                        },
                    })
                    .collect(),
            })
            .collect(),
    })
}
impl PayloadCodec for JsCodec {
    type Payload = JsValue;
    type Error = CodecError;
    fn format_id(&self) -> &str {
        &self.format
    }
    fn encode_commit(&self, payload: &JsValue) -> Result<Vec<u8>, CodecError> {
        self.bytes("encodeCommit", &[payload.clone()])
    }
    fn decode_commit(&self, bytes: &[u8], limits: &DecodeLimits) -> Result<JsValue, CodecError> {
        self.invoke(
            "decodeCommit",
            &[
                Uint8Array::from(bytes).into(),
                encode(&Limits::from(limits))?,
            ],
        )
    }
    fn encode_fragment(
        &self,
        s: &FragmentShape,
        payloads: &[&JsValue],
    ) -> Result<Vec<u8>, CodecError> {
        let values = Array::new();
        for payload in payloads {
            values.push(payload);
        }
        self.bytes("encodeFragment", &[shape(s)?, values.into()])
    }
    fn decode_fragment(
        &self,
        s: &FragmentShape,
        bytes: &[u8],
        limits: &DecodeLimits,
    ) -> Result<Vec<JsValue>, CodecError> {
        let value = self.invoke(
            "decodeFragment",
            &[
                shape(s)?,
                Uint8Array::from(bytes).into(),
                encode(&Limits::from(limits))?,
            ],
        )?;
        if !Array::is_array(&value) {
            return Err(CodecError(
                Error::input("decodeFragment must return an array").into(),
            ));
        }
        let array = Array::from(&value);
        if array.length() as usize != s.members.len() {
            return Err(CodecError(
                Error::input("decodeFragment returned wrong member count").into(),
            ));
        }
        Ok(array.iter().collect())
    }
}

fn item(value: stockfoil::EncodedItem) -> Result<JsValue, JsValue> {
    #[derive(Serialize)]
    #[serde(tag = "kind", rename_all = "camelCase")]
    enum Metadata {
        Commit {
            hash: String,
            parents: Vec<String>,
        },
        Fragment {
            head: String,
            boundary: Vec<String>,
            checkpoints: Vec<serde_bytes::ByteBuf>,
        },
    }
    let metadata = match value.metadata {
        stockfoil::TransportMetadata::Commit { hash, parents } => Metadata::Commit {
            hash: dto::hex(hash.as_slice()),
            parents: parents.iter().map(|h| dto::hex(h.as_slice())).collect(),
        },
        stockfoil::TransportMetadata::Fragment { range, checkpoints } => Metadata::Fragment {
            head: dto::hex(range.head.as_slice()),
            boundary: range
                .boundary
                .iter()
                .map(|h| dto::hex(h.as_slice()))
                .collect(),
            checkpoints: checkpoints
                .into_iter()
                .map(|c| serde_bytes::ByteBuf::from(c.0.to_vec()))
                .collect(),
        },
    };
    let result: JsValue = js_sys::Object::new().into();
    set(&result, "metadata", &encode(&metadata)?)?;
    set(&result, "bytes", &Uint8Array::from(value.bytes.as_slice()))?;
    Ok(result)
}
fn commit(meta: &stockfoil::CommitMeta, payload: JsValue) -> Result<JsValue, JsValue> {
    let value = encode(&dto::Commit::from_meta(meta, &[]))?;
    set(&value, "payload", &payload)?;
    Ok(value)
}

/// Use only nonblocking try_lock, including for reads: callbacks can re-enter.
/// A mutex also poisons on an unexpected native panic, preserving wasm-bindgen's
/// RefUnwindSafe guarantee without asserting that partially mutated state is safe.
#[wasm_bindgen]
pub struct RawCodecGraph {
    graph: Mutex<stockfoil::CommitGraph<JsCodec>>,
    // Copy callback retained separately so materialization need not expose codec.
    copier: JsCodec,
}
impl RawCodecGraph {
    fn read(&self) -> Result<MutexGuard<'_, stockfoil::CommitGraph<JsCodec>>, JsValue> {
        self.write()
    }
    fn write(&self) -> Result<MutexGuard<'_, stockfoil::CommitGraph<JsCodec>>, JsValue> {
        self.graph.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => {
                Error::new("ReentrantCall", "graph operation in progress").into()
            }
            TryLockError::Poisoned(_) => {
                Error::new("InternalError", "graph unavailable after native panic").into()
            }
        })
    }
}
#[wasm_bindgen]
impl RawCodecGraph {
    #[wasm_bindgen(constructor)]
    pub fn new(bridge: JsValue, format: String, limits: JsValue) -> Result<Self, JsValue> {
        let limits: Limits = decode(limits)?;
        Ok(Self {
            graph: Mutex::new(stockfoil::CommitGraph::with_limits(
                JsCodec {
                    bridge: bridge.clone(),
                    format: format.clone(),
                },
                limits.into(),
            )),
            copier: JsCodec { bridge, format },
        })
    }
    pub fn commit(&self, author: String, payload: JsValue) -> Result<JsValue, JsValue> {
        let mut graph = self.write()?;
        let payload = self.copier.copy(&payload)?;
        let draft = graph
            .commit(dto::author(&author)?, payload)
            .map_err(core_error)?;
        let result: JsValue = js_sys::Object::new().into();
        set(&result, "hash", &dto::hex(draft.hash.as_slice()).into())?;
        set(
            &result,
            "signingBytes",
            &Uint8Array::from(draft.signing_bytes.as_slice()),
        )?;
        set(
            &result,
            "report",
            &encode(&dto::Report::from(draft.report))?,
        )?;
        Ok(result)
    }
    #[wasm_bindgen(js_name = describeCommit)]
    pub fn describe_commit(&self, author: String, payload: JsValue) -> Result<JsValue, JsValue> {
        let graph = self.write()?;
        let payload = self.copier.copy(&payload)?;
        let meta = graph
            .describe_commit(dto::author(&author)?, &payload)
            .map_err(core_error)?;
        commit(&meta, payload)
    }
    #[wasm_bindgen(js_name = addSignature)]
    pub fn add_signature(&self, evidence: JsValue) -> Result<JsValue, JsValue> {
        let (author, commit, signature) = decode::<dto::Evidence>(evidence)?.into_core()?;
        let report = self
            .write()?
            .ingest_signature(author, commit, signature)
            .map_err(Error::from)?;
        encode(&dto::Report::from(report))
    }
    #[wasm_bindgen(js_name = setDecodeLimits)]
    pub fn set_decode_limits(&self, limits: JsValue) -> Result<(), JsValue> {
        let mut graph = self.write()?;
        let limits: Limits = decode(limits)?;
        graph.set_decode_limits(limits.into());
        Ok(())
    }
    pub fn ingest(&self, bytes: Vec<u8>) -> Result<JsValue, JsValue> {
        let report = self.write()?.ingest(&bytes).map_err(core_error)?;
        encode(&dto::Report::from(report))
    }
    #[wasm_bindgen(js_name = exportChanges)]
    pub fn export_changes(&self, since: &RawExportCursor) -> Result<JsValue, JsValue> {
        let (changes, cursor) = self.write()?.export_changes(&since.0).map_err(core_error)?;
        let added = Array::new();
        for value in changes.added {
            added.push(&item(value)?);
        }
        let retired: Vec<dto::Id> = changes.retired.into_iter().map(Into::into).collect();
        let result: JsValue = js_sys::Object::new().into();
        set(&result, "added", &added)?;
        set(&result, "retired", &encode(&retired)?)?;
        set(&result, "cursor", &RawExportCursor(cursor).into())?;
        Ok(result)
    }
    pub fn changes(
        &self,
        since: &RawMaterializationCursor,
        view: JsValue,
    ) -> Result<JsValue, JsValue> {
        let graph = self.write()?;
        let target = if view.is_undefined() {
            graph.view_point()
        } else {
            let permissions = js_sys::Reflect::get(&view, &"writePermissions".into())
                .map_err(|_| Error::input("expected a viewpoint with writePermissions"))?;
            require_map(&permissions)?;
            decode::<dto::View>(view)?.into_core()?
        };
        let mut changes = Vec::new();
        let cursor = graph
            .visit_changes(&since.0, &target, |change| {
                changes.push((|| {
                    let result: JsValue = js_sys::Object::new().into();
                    set(
                        &result,
                        "commit",
                        &commit(change.meta, self.copier.copy(change.payload)?)?,
                    )?;
                    set(&result, "newlyDelivered", &change.newly_delivered.into())?;
                    set(
                        &result,
                        "visibility",
                        &JsValue::from_str(match change.visibility {
                            Vis::Before => "before",
                            Vis::After => "after",
                            Vis::Both => "both",
                            Vis::Neither => "neither",
                        }),
                    )?;
                    Ok::<_, JsValue>(result)
                })());
            })
            .map_err(Error::from)?;
        let values = Array::new();
        for change in changes {
            values.push(&change?);
        }
        let result: JsValue = js_sys::Object::new().into();
        set(&result, "changes", &values)?;
        set(&result, "cursor", &RawMaterializationCursor(cursor).into())?;
        Ok(result)
    }
    #[wasm_bindgen(js_name = viewPoint)]
    pub fn view_point(&self) -> Result<JsValue, JsValue> {
        encode(&dto::View::from(self.read()?.view_point()))
    }
    #[wasm_bindgen(js_name = signedViewPoint)]
    pub fn signed_view_point(&self) -> Result<JsValue, JsValue> {
        encode(&dto::View::from(self.read()?.signed_view_point()))
    }
    #[wasm_bindgen(js_name = setWritePermissions)]
    pub fn set_write_permissions(&self, permissions: JsValue) -> Result<(), JsValue> {
        require_map(&permissions)?;
        self.write()?
            .set_write_permissions(&dto::permissions(decode(permissions)?)?);
        Ok(())
    }
    #[wasm_bindgen(js_name = hasCommit)]
    pub fn has_commit(&self, hash: String) -> Result<bool, JsValue> {
        Ok(self.read()?.has_commit(&dto::hash(&hash)?))
    }
    #[wasm_bindgen(js_name = isAncestor)]
    pub fn is_ancestor(&self, ancestor: String, descendant: String) -> Result<bool, JsValue> {
        Ok(self
            .read()?
            .is_ancestor(&dto::hash(&ancestor)?, &dto::hash(&descendant)?))
    }
    #[wasm_bindgen(js_name = pendingItems)]
    pub fn pending_items(&self) -> Result<usize, JsValue> {
        Ok(self.read()?.pending_items())
    }
    #[wasm_bindgen(js_name = missingDependencies)]
    pub fn missing_dependencies(&self) -> Result<JsValue, JsValue> {
        let mut hashes: Vec<_> = self
            .read()?
            .missing_dependencies()
            .map(|h| dto::hex(h.as_slice()))
            .collect();
        hashes.sort_unstable();
        encode(&hashes)
    }
}
