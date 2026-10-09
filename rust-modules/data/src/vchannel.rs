//! **Virtual channels**: TV channels the app makes out of the viewer's own Plex library and airs
//! itself, beside the Tunarr channels in the Live TV guide.
//!
//! * [`schedule`] — a channel's timeline, computed from its recipe (programmes, order, seed,
//!   start): what is on at any instant, and a window of guide.
//! * [`build`] — a recipe's programmes, from its source and rules.
//! * [`catalog`] — the movie and TV libraries with the facts rules and suggestions read.
//! * [`recipe`] — what a channel is made of, and how it rides in a Plex playlist's description so
//!   every television in the house shares it.

pub mod build;
pub mod catalog;
pub mod recipe;
pub mod schedule;
