//! Private Wasm ABI for the handwritten JavaScript wrapper.
//! Native envelopes are validated by the single codec-owning graph.
mod codec;
mod dto;
mod error;

use serde::{Serialize, de::DeserializeOwned};
use stockfoil::{CommitContents, CommitMeta};
use wasm_bindgen::prelude::*;

use error::Error;

fn decode<T: DeserializeOwned>(value: JsValue) -> Result<T, Error> {
    serde_wasm_bindgen::from_value(value).map_err(Error::input)
}

fn encode(value: &impl Serialize) -> Result<JsValue, JsValue> {
    // Maps and byte arrays are owned, never borrowed Wasm memory.
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

/// Describe untrusted byte-payload metadata, without authenticating it.
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
