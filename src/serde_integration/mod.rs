//! Serde integration for rust-yaml.
//!
//! Provides a full serde data format (Serializer + Deserializer) and
//! impls of `Serialize` / `Deserialize` for [`crate::Value`].
//!
//! All items are gated by the `serde` feature.

#[cfg(feature = "serde")]
mod de;
#[cfg(feature = "serde")]
mod error;
#[cfg(feature = "serde")]
mod ser;
#[cfg(feature = "serde")]
mod value;

// TODO(#21 Task 13): Uncomment when public API lands.
// These will be uncommented after the implementing functions are added.
// #[cfg(feature = "serde")]
// pub use de::{from_reader, from_slice, from_str};
// #[cfg(feature = "serde")]
// pub use ser::{to_string, to_writer};
