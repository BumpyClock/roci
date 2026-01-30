//! Image input types.

/// An image that can be sent to a model.
#[derive(Debug, Clone)]
pub enum ImageInput {
    /// Base64-encoded image data with MIME type.
    Base64 { data: String, mime_type: String },
    /// URL pointing to an image.
    Url(String),
    /// Raw bytes with MIME type.
    Bytes { data: Vec<u8>, mime_type: String },
}

impl ImageInput {
    /// Create from base64 string.
    pub fn from_base64(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self::Base64 {
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }

    /// Create from URL.
    pub fn from_url(url: impl Into<String>) -> Self {
        Self::Url(url.into())
    }

    /// Create from raw bytes.
    pub fn from_bytes(data: Vec<u8>, mime_type: impl Into<String>) -> Self {
        Self::Bytes {
            data,
            mime_type: mime_type.into(),
        }
    }

    /// Convert to base64 data string regardless of variant.
    pub fn to_base64(&self) -> String {
        match self {
            Self::Base64 { data, .. } => data.clone(),
            Self::Url(_) => String::new(), // URLs sent as-is, no conversion
            Self::Bytes { data, .. } => {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(data)
            }
        }
    }

    /// Get MIME type if available.
    pub fn mime_type(&self) -> Option<&str> {
        match self {
            Self::Base64 { mime_type, .. } | Self::Bytes { mime_type, .. } => Some(mime_type),
            Self::Url(_) => None,
        }
    }
}
