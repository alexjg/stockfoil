//! Private Wasm ABI for the handwritten JavaScript wrapper.
//! All transport validation happens here, before graph or queue mutation.
mod dto;
mod error;

use serde::{Serialize, de::DeserializeOwned};
use stockfoil::{CommitContents, CommitMeta, Vis};
use wasm_bindgen::prelude::*;

use error::Error;

fn decode<T: DeserializeOwned>(value: JsValue) -> Result<T, Error> {
    serde_wasm_bindgen::from_value(value).map_err(Error::input)
}

fn encode(value: &impl Serialize) -> Result<JsValue, JsValue> {
    // Default serializer keeps HashMaps as JS Maps and serde_bytes as owned
    // Uint8Arrays (never a borrowed view of Wasm linear memory).
    serde_wasm_bindgen::to_value(value).map_err(|e| Error::new("InternalError", e).into())
}

fn require_map(value: &JsValue) -> Result<(), Error> {
    if !value.is_instance_of::<js_sys::Map>() {
        return Err(Error::input("writePermissions must be a Map"));
    }
    Ok(())
}

fn set(object: &JsValue, name: &str, value: &JsValue) -> Result<(), JsValue> {
    js_sys::Reflect::set(object, &name.into(), value)
        .map_err(|_| JsValue::from(Error::new("InternalError", "failed to construct result")))?;
    Ok(())
}

#[wasm_bindgen]
pub struct RawMaterializationCursor(stockfoil::MaterializationCursor);

#[wasm_bindgen]
impl RawMaterializationCursor {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self(stockfoil::MaterializationCursor::new())
    }

    #[wasm_bindgen(js_name = clone)]
    pub fn clone_cursor(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Default for RawMaterializationCursor {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
pub struct RawExportCursor(stockfoil::ExportCursor);

#[wasm_bindgen]
impl RawExportCursor {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self(stockfoil::ExportCursor::default())
    }

    #[wasm_bindgen(js_name = clone)]
    pub fn clone_cursor(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Default for RawExportCursor {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
pub struct RawGraph(stockfoil::CommitGraph<Vec<u8>>);

#[wasm_bindgen]
impl RawGraph {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self(stockfoil::CommitGraph::new())
    }

    pub fn commit(&mut self, author: String, payload: Vec<u8>) -> Result<JsValue, JsValue> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Draft {
            hash: String,
            #[serde(with = "serde_bytes")]
            signing_bytes: Vec<u8>,
            report: dto::Report,
        }
        let meta = self.0.commit_by(dto::author(&author)?, &payload);
        let hash = dto::hex(meta.hash().as_slice());
        let signing_bytes = meta.hash().as_slice().to_vec();
        let report = self.0.ingest_local_commit(meta, payload).into();
        encode(&Draft {
            hash,
            signing_bytes,
            report,
        })
    }

    #[wasm_bindgen(js_name = addSignature)]
    pub fn add_signature(&mut self, evidence: JsValue) -> Result<JsValue, JsValue> {
        let evidence = decode::<dto::Evidence>(evidence)?.into_core()?;
        let report = self
            .0
            .ingest_signature(evidence.author, evidence.commit, evidence.signature)
            .map_err(Error::from)?;
        encode(&dto::Report::from(report))
    }

    pub fn ingest(&mut self, item: JsValue) -> Result<JsValue, JsValue> {
        let dto::PreparedItem {
            verified,
            mut payloads,
        } = decode::<dto::TransportItem>(item)?.verify()?;
        let report = self
            .0
            .ingest(verified, |meta| {
                // Verification derives these members from exactly this owned map.
                // Core invokes the callback at most once per not-yet-delivered member.
                payloads
                    .remove(meta.hash())
                    .expect("verified member has owned payload")
            })
            .map_err(Error::from)?;
        encode(&dto::Report::from(report))
    }

    #[wasm_bindgen(js_name = viewPoint)]
    pub fn view_point(&self) -> Result<JsValue, JsValue> {
        encode(&dto::View::from(self.0.view_point()))
    }

    #[wasm_bindgen(js_name = signedViewPoint)]
    pub fn signed_view_point(&self) -> Result<JsValue, JsValue> {
        encode(&dto::View::from(self.0.signed_view_point()))
    }

    #[wasm_bindgen(js_name = setWritePermissions)]
    pub fn set_write_permissions(&mut self, permissions: JsValue) -> Result<(), JsValue> {
        require_map(&permissions)?;
        let permissions = dto::permissions(decode(permissions)?)?;
        self.0.set_write_permissions(&permissions);
        Ok(())
    }

    pub fn changes(
        &self,
        since: &RawMaterializationCursor,
        view: JsValue,
    ) -> Result<JsValue, JsValue> {
        let target = if view.is_undefined() {
            self.0.view_point()
        } else {
            let permissions = js_sys::Reflect::get(&view, &"writePermissions".into())
                .map_err(|_| Error::input("invalid viewpoint"))?;
            require_map(&permissions)?;
            decode::<dto::View>(view)?.into_core()?
        };
        let mut changes = Vec::new();
        let cursor = self
            .0
            .visit_changes(&since.0, &target, |change| {
                changes.push(dto::Change {
                    commit: dto::Commit::from_meta(change.meta, change.payload),
                    newly_delivered: change.newly_delivered,
                    visibility: match change.visibility {
                        Vis::Before => "before",
                        Vis::After => "after",
                        Vis::Both => "both",
                        Vis::Neither => "neither",
                    },
                });
            })
            .map_err(Error::from)?;
        let result: JsValue = js_sys::Object::new().into();
        set(&result, "changes", &encode(&changes)?)?;
        set(&result, "cursor", &RawMaterializationCursor(cursor).into())?;
        Ok(result)
    }

    #[wasm_bindgen(js_name = exportChanges)]
    pub fn export_changes(&self, since: &RawExportCursor) -> Result<JsValue, JsValue> {
        let (changes, cursor) = self.0.export_changes(&since.0);
        let added = changes
            .added
            .into_iter()
            .map(|item| dto::TransportItem::from_core(item, &self.0))
            .collect::<Result<Vec<_>, _>>()?;
        let retired: Vec<dto::Id> = changes.retired.into_iter().map(Into::into).collect();
        let result: JsValue = js_sys::Object::new().into();
        set(&result, "added", &encode(&added)?)?;
        set(&result, "retired", &encode(&retired)?)?;
        set(&result, "cursor", &RawExportCursor(cursor).into())?;
        Ok(result)
    }

    #[wasm_bindgen(js_name = hasCommit)]
    pub fn has_commit(&self, hash: String) -> Result<bool, JsValue> {
        Ok(self.0.has_commit(&dto::hash(&hash)?))
    }

    #[wasm_bindgen(js_name = isAncestor)]
    pub fn is_ancestor(&self, ancestor: String, descendant: String) -> Result<bool, JsValue> {
        Ok(self
            .0
            .is_ancestor(&dto::hash(&ancestor)?, &dto::hash(&descendant)?))
    }

    #[wasm_bindgen(js_name = pendingItems)]
    pub fn pending_items(&self) -> usize {
        self.0.pending_items()
    }

    #[wasm_bindgen(js_name = missingDependencies)]
    pub fn missing_dependencies(&self) -> Result<JsValue, JsValue> {
        let mut hashes: Vec<_> = self
            .0
            .missing_dependencies()
            .map(|h| dto::hex(h.as_slice()))
            .collect();
        hashes.sort_unstable();
        encode(&hashes)
    }

    #[wasm_bindgen(js_name = missingAuthentication)]
    pub fn missing_authentication(&self) -> Result<JsValue, JsValue> {
        let mut hashes: Vec<_> = self
            .0
            .missing_authentication()
            .map(|h| dto::hex(h.as_slice()))
            .collect();
        hashes.sort_unstable();
        encode(&hashes)
    }
}

impl Default for RawGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen(js_name = authorId)]
pub fn author_id(bytes: Vec<u8>) -> Result<String, JsValue> {
    let author = stockfoil::Author::try_from(bytes.as_slice()).map_err(Error::input)?;
    Ok(dto::hex(author.as_slice()))
}

#[wasm_bindgen(js_name = commitHash)]
pub fn commit_hash(bytes: Vec<u8>) -> Result<String, JsValue> {
    let hash = stockfoil::CommitHash::try_from(bytes.as_slice()).map_err(Error::input)?;
    Ok(dto::hex(hash.as_slice()))
}

/// Reconstruct derived hashes when decoding a format containing only contents.
/// This describes untrusted metadata; it does not establish signature validity.
#[wasm_bindgen(js_name = describeCommit)]
pub fn describe_commit(
    author: String,
    parents: JsValue,
    payload: Vec<u8>,
) -> Result<JsValue, JsValue> {
    let meta = CommitMeta::new(CommitContents {
        author: dto::author(&author)?,
        parents: dto::hashes(decode(parents)?)?,
        payload: stockfoil::BlobHash::of_contents(&payload),
    });
    encode(&dto::Commit::from_meta(&meta, &payload))
}
