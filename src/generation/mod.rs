//! Text, streaming, and structured output generation.

pub mod text;
pub mod stream;
pub mod object;
pub mod convenience;

pub use text::generate_text;
pub use stream::stream_text;
pub use object::{generate_object, stream_object};
pub use convenience::{generate, stream, analyze};
