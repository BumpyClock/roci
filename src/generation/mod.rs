//! Text, streaming, and structured output generation.

pub mod convenience;
pub mod object;
pub mod stream;
pub mod text;

pub use convenience::{analyze, generate, stream};
pub use object::{generate_object, stream_object};
pub use stream::{stream_text, stream_text_with_tools};
pub use text::generate_text;
