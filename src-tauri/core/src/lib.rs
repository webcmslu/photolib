//! Phototek's library, shared by the desktop app and the NAS server: the
//! index of a photo folder, thumbnails, faces and people, memories and
//! trips, the map and search. Nothing here knows about a user interface.

pub mod collections;
pub mod db;
pub mod duplicates;
pub mod enhance;
pub mod export;
pub mod faces;
pub mod fingerprint;
pub mod frames;
pub mod geo;
#[cfg(target_os = "macos")]
pub mod heic_macos;
pub mod indexer;
pub mod metadata;
pub mod org;
pub mod original;
pub mod people;
pub mod places;
pub mod raw;
pub mod search;
pub mod similar;
pub mod thumbs;
pub mod video;
