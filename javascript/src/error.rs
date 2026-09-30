use serde::Serialize;
use wasm_bindgen::JsValue;

/// Serializable separately so deferred rejections are plain records, not handles.
#[derive(Debug, Serialize)]
pub(crate) struct Error {
    pub code: &'static str,
    pub message: String,
}

impl Error {
    pub fn new(code: &'static str, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }

    pub fn input(message: impl ToString) -> Self {
        Self::new("InvalidInput", message)
    }
}

impl From<Error> for JsValue {
    fn from(error: Error) -> Self {
        let value = js_sys::Error::new(&error.message);
        value.set_name("StockfoilError");
        // This is a fresh ordinary Error; setting an own property cannot fail.
        js_sys::Reflect::set(&value, &"code".into(), &error.code.into())
            .expect("fresh Error accepts code property");
        value.into()
    }
}

impl From<stockfoil::SignatureError> for Error {
    fn from(error: stockfoil::SignatureError) -> Self {
        use stockfoil::SignatureError::*;
        let code = match &error {
            NotPendingLocalCommit(_) => "NotPendingLocalCommit",
            InvalidPublicKey(_) => "InvalidPublicKey",
            InvalidSignature { .. } => "InvalidSignature",
        };
        Self::new(code, error)
    }
}

impl From<stockfoil::IngestError> for Error {
    fn from(error: stockfoil::IngestError) -> Self {
        match error {
            stockfoil::IngestError::InvalidFragment => Self::new("InvalidFragment", error),
        }
    }
}

impl From<stockfoil::DiffError> for Error {
    fn from(error: stockfoil::DiffError) -> Self {
        Self::new("MissingCommit", error)
    }
}
