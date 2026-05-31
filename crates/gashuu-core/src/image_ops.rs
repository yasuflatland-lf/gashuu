#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn decode(_bytes: &[u8]) -> Result<DecodedImage, crate::error::CoreError> {
    unimplemented!()
}
