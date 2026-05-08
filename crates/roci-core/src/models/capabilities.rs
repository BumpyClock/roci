//! Model capabilities descriptor.

use serde::{Deserialize, Serialize};

const DEFAULT_IMAGE_MAX_IMAGES: usize = 20;
const DEFAULT_IMAGE_MAX_BYTES: usize = 20 * 1024 * 1024;
const DEFAULT_IMAGE_MAX_TOTAL_BYTES: usize = 50 * 1024 * 1024;
const DEFAULT_IMAGE_TOKEN_ESTIMATE: usize = 1200;

/// Describes what a model can do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCapabilities {
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub supports_streaming: bool,
    pub supports_json_mode: bool,
    pub supports_json_schema: bool,
    pub supports_reasoning: bool,
    pub supports_system_messages: bool,
    pub context_length: usize,
    pub max_output_tokens: Option<usize>,
    /// Model input constraints across text, image, and file channels used for preflight validation.
    pub input: ModelInputCapabilities,
}

/// Provider-independent model input limits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelInputCapabilities {
    pub text: TextInputCapabilities,
    pub image: Option<ImageInputCapabilities>,
    pub file: FileInputCapabilities,
}

impl ModelInputCapabilities {
    /// Build model input capabilities with image limits enabled when vision is supported.
    pub fn from_vision_support(supports_vision: bool) -> Self {
        Self {
            image: supports_vision.then(ImageInputCapabilities::default),
            ..Self::default()
        }
    }
}

/// Text input limits after attachment resolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TextInputCapabilities {
    pub max_text_bytes: Option<usize>,
    pub max_text_tokens: Option<usize>,
}

/// Image input limits after attachment resolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageInputCapabilities {
    pub max_images: usize,
    pub max_image_bytes: Option<usize>,
    pub max_total_image_bytes: Option<usize>,
    pub supported_mime_types: Vec<String>,
    pub image_token_estimate: usize,
}

impl Default for ImageInputCapabilities {
    fn default() -> Self {
        Self {
            max_images: DEFAULT_IMAGE_MAX_IMAGES,
            max_image_bytes: Some(DEFAULT_IMAGE_MAX_BYTES),
            max_total_image_bytes: Some(DEFAULT_IMAGE_MAX_TOTAL_BYTES),
            supported_mime_types: vec![
                "image/png".to_string(),
                "image/jpeg".to_string(),
                "image/webp".to_string(),
                "image/gif".to_string(),
            ],
            image_token_estimate: DEFAULT_IMAGE_TOKEN_ESTIMATE,
        }
    }
}

/// File input limits. Native file payload transport is intentionally disabled by default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct FileInputCapabilities {
    pub native_file_input: bool,
    pub max_files: usize,
    pub max_file_bytes: Option<usize>,
    pub max_total_file_bytes: Option<usize>,
    pub supported_mime_types: Vec<String>,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            supports_vision: false,
            supports_tools: false,
            supports_streaming: true,
            supports_json_mode: false,
            supports_json_schema: false,
            supports_reasoning: false,
            supports_system_messages: true,
            context_length: 4096,
            max_output_tokens: None,
            input: ModelInputCapabilities::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_capabilities_have_text_only_input() {
        let caps = ModelCapabilities::default();

        assert!(!caps.supports_vision);
        assert_eq!(caps.input, ModelInputCapabilities::default());
        assert!(caps.input.image.is_none());
        assert!(!caps.input.file.native_file_input);
    }

    #[test]
    fn vision_input_defaults_are_catalog_ready() {
        let input = ModelInputCapabilities::from_vision_support(true);
        let image = input.image.expect("vision input");

        assert_eq!(image.max_images, 20);
        assert_eq!(image.max_image_bytes, Some(20 * 1024 * 1024));
        assert_eq!(image.max_total_image_bytes, Some(50 * 1024 * 1024));
        assert_eq!(image.image_token_estimate, 1200);
        assert_eq!(
            image.supported_mime_types,
            vec![
                "image/png".to_string(),
                "image/jpeg".to_string(),
                "image/webp".to_string(),
                "image/gif".to_string(),
            ]
        );
    }

    #[test]
    fn text_only_input_has_no_image_support() {
        assert_eq!(
            ModelInputCapabilities::from_vision_support(false),
            ModelInputCapabilities::default()
        );
    }

    #[test]
    fn input_capabilities_round_trip_through_json() {
        let caps = ModelCapabilities {
            supports_vision: true,
            input: ModelInputCapabilities::from_vision_support(true),
            ..ModelCapabilities::default()
        };

        let json = serde_json::to_string(&caps).expect("serialize capabilities");
        let decoded: ModelCapabilities =
            serde_json::from_str(&json).expect("deserialize capabilities");

        assert_eq!(decoded, caps);
    }
}
