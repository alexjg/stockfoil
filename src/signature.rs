#[derive(Clone, Copy)]
pub struct Signature([u8; 64]);

impl Signature {
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }
}

impl From<[u8; 64]> for Signature {
    fn from(value: [u8; 64]) -> Self {
        Self(value)
    }
}

impl From<Signature> for [u8; 64] {
    fn from(value: Signature) -> Self {
        value.0
    }
}
