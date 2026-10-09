//! **Virtual channels**: TV channels the app makes out of the viewer's own Plex library and airs
//! itself, beside the Tunarr channels in the Live TV guide.
//!
//! * [`schedule`] — a channel's timeline, computed from its recipe (programmes, order, seed,
//!   start): what is on at any instant, and a window of guide.
//! * [`build`] — a recipe's programmes, from its source and rules.
//! * [`channels`] — the channels the profile holds, live: list, build, preview, keep, update, delete.
//! * [`dismissed`] — the suggestions each profile said it is not interested in.
//! * [`catalog`] — the movie and TV libraries with the facts rules and suggestions read.
//! * [`remote`] — the server side: list, keep, update and delete channel playlists; read sources.
//! * [`suggest`] — suggested channels: ideas from fourteen angles, measured, scored for the profile
//!   and the moment, chosen for variety, named and explained.
//! * [`taste`] — what the profile likes, from its own view state: the suggestions' compass.
//! * [`recipe`] — what a channel is made of, and how it rides in a Plex playlist's description so
//!   every television in the house shares it.

pub mod build;
pub mod catalog;
pub mod channels;
pub mod dismissed;
pub mod recipe;
pub mod remote;
pub mod schedule;
pub mod suggest;
pub mod taste;
