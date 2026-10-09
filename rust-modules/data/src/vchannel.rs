//! **Virtual channels**: TV channels the app makes out of the viewer's own Plex library and airs
//! itself, beside the Tunarr channels in the Live TV guide.
//!
//! * [`schedule`] — a channel's timeline, computed from its recipe (programmes, order, seed,
//!   start): what is on at any instant, and a window of guide.

pub mod schedule;
