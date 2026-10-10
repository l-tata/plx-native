# Live TV through Tunarr — plan and status

**Status: Phases 1–3 implemented, not yet verified on the television (Phase 0 was skipped).**
What landed, by crate: SSDP and the HDHomeRun/XMLTV client, lineup/guide join and the `LiveTv`
store (`plx_net::ssdp`, `plx_data::livetv`, `plx_data::stores::livetv`); the channel stream probe
and the live playback session (`plx_media::live`, `route::install_live_stream`); the Live TV page —
setup face, guide grid, CH▲/▼ paging and digit entry (`plx_screens::livetv`); the player's live
banner, CH▲/▼, digits, last channel, a channel list to surf over the playing channel (UP/DOWN) and bounded automatic re-tune (`plx_appkit::live_banner`,
`screens::player`, `app::livetv`); Settings > Live TV; and the Live TV pill on the tab strip once there is
anything to show (a Tunarr server, or since 0.12.0 a virtual channel or a suggestion). Home's On Now shelf has since landed (`plx_data::livetv::on_now`). Not built yet: a "now playing" item on Home, the airing
popover, favourites/ordering, and a re-Load on a
mid-stream frame-rate change (risk 2 — run Tunarr with frame-rate normalisation on). Wall times
print in the C library's zone; when the library says UTC, in the set's own zone from its system
clock service, else in the zone Tunarr writes XMLTV in (`plx_base::wallclock`). The guide opens on
the channel last tuned (`Session::livetv_channel`).

Since 0.12.0 Live TV also airs **virtual channels** the app makes from the viewer's own library,
with or without Tunarr — see [Virtual channels](#virtual-channels) at the end.

## The decision

PlxNative gets Live TV by being a client of **[Tunarr](https://github.com/chrisbenincasa/tunarr)**,
exactly as Plex, Jellyfin and IPTV players are: Tunarr presents itself as an HDHomeRun tuner and
publishes an XMLTV guide, and we consume those two surfaces directly. Plex Media Server is not in
the path — no `/livetv/*`, no Plex DVR, no Plex Pass. Tunarr builds its channels from the same
libraries this app already browses, so nothing Plex offers here is lost that this path needed.

Not in scope: Plex's free FAST channels, Plex DVR, recording, and a real (broadcast) HDHomeRun.
Because the client speaks only the HDHomeRun HTTP surface plus XMLTV, a broadcast HDHomeRun would
reach the same code later, but its MPEG-2 video is a separate problem this plan does not take on.

## What Tunarr exposes

Read from Tunarr's server source at `7a76d1d5` (2026-10-08, `1.2.0-dev.1`). Paths are relative to
the Tunarr origin, default port 8000 (`http://192.0.2.20:8000` below is a placeholder).

| Surface | Where in Tunarr | What it gives us |
|---|---|---|
| `GET /discover.json` | `services/HDHRService.ts` `getHdhrDevice` | `FriendlyName`, `DeviceID: "Tunarr"`, `TunerCount` (advertised only, default 2, **not enforced**), `BaseURL`, `LineupURL` |
| `GET /lineup.json` | `api/hdhrApi.ts` | `[{GuideNumber, GuideName, URL}]`, one per non-stealth channel. `URL` is always `{origin}/stream/channels/{uuid}.ts`, whatever the channel's own stream mode. An empty Tunarr returns one placeholder entry pointing at `/setup` |
| `GET /lineup_status.json` | `api/hdhrApi.ts` | constant `{ScanInProgress:0, ScanPossible:1, Source:"Cable"}`; nothing to read |
| `GET /api/xmltv.xml` | `api/index.ts`, `services/XmlTvWriter.ts` | the guide (below). `Cache-Control: no-store`, `{{host}}` rewritten to the requesting origin |
| `GET /api/channels.m3u` | `services/M3UService.ts` | the IPTV view of the same channels; not needed (lineup.json carries the same facts) |
| `GET /stream/channels/{uuid}.ts` | `api/streamApi.ts` | one continuous MPEG-TS stream (`Content-Type: video/mp2t`) for as long as the connection stays open |
| SSDP | `services/HDHRService.ts` (`node-ssdp`) | advertises `upnp:rootdevice` and `urn:schemas-upnp-org:device:MediaServer:1`, `LOCATION` → `/device.xml`, fixed UDN `uuid:d936e232-6671-4cd7-a8ab-34b5956ff4d6`. On by default (`autoDiscoveryEnabled`) |

**No authentication on any of these.** The HDHR, stream and XMLTV/M3U routes set
`authRequired: false`, so no credential is involved and nothing here touches the app's token
handling.

### The guide

- One `<channel>` per channel. `id` is `C{number}.{digit-code}.tunarr.com` (`util/channels.ts`
  `getChannelId`); `display-name` appears three times: `"{number} {name}"`, `"{number}"`,
  `"{name}"`. One `<icon src>` (the channel's icon, or `{origin}/images/tunarr.png`).
- **The join to `lineup.json` is the channel number.** `GuideNumber` is `channel.number`, and the
  bare-number `display-name` is the same value. Match on that, which is what HDHomeRun clients do;
  do not try to rebuild the `id` scheme.
- `<programme start stop channel>` with `title`, optional `sub-title` (episode title), `desc`,
  `credits`, `date`, `category`, `length`, `icon`/`image` (artwork served from the Tunarr origin),
  `episode-num` in both `onscreen` (`S01E02`) and `xmltv_ns` forms, and `rating`. Times are the
  XMLTV `YYYYMMDDhhmmss ±zzzz` form.
- Filler ("flex") slots are programmes too, titled with the channel's flex title, so the guide has
  no gaps to invent.
- Window: `programmingHours` (default **12 h**), rebuilt every `refreshHours` (default **4 h**).
  At the default this is a small document; a large install at a long horizon can reach megabytes,
  so it is parsed as a stream, not loaded as a tree.

### The stream

- `mpegts_concat` mode: Tunarr's own FFmpeg concatenates each programme's transcode into one TS
  (`stream/ConcatStream.ts`, `ffmpeg/FfmpegStreamFactory.ts` `createConcatSession`). Every
  programme is transcoded to the channel's transcode config, so resolution and codecs stay constant
  across programme boundaries.
- **Default output is H.264 + AAC stereo at 2000 kbps** (`db/schema/TranscodeConfig.ts`), with
  `deinterlaceVideo: true` and **`normalizeFrameRate: false`**. H.264 and ADTS-AAC are both formats
  this app already feeds to the television. Frame rate is the one property that can change
  mid-stream (a 23.976 film followed by a 29.97 episode); see "Recommended Tunarr settings".
- Session lifecycle (`stream/Session.ts`, `stream/SessionManager.ts`): closing the connection
  removes that connection; the channel's session lingers 5–15 s before cleanup. Zapping back to a
  channel inside that window reattaches to a running transcode, so it starts faster than a cold
  tune. An optional `?token=<uuid>` names the connection; a reconnect with the same token
  is the same viewer.
- Start-up cost is Tunarr starting FFmpeg for that channel: seconds, and dominated by the server's
  transcode speed, not by anything the client does.

## What this app already has

- **The bytes path.** `net/src/stream.rs` reads response bodies with no `Content-Length` (PMS's
  progressive transcode is one), and the bundled FFmpeg already includes the `mpegts` demuxer
  (`ci/build-ffmpeg.sh`) and the `h264`/`aac` parsers. What it lacked until v0.10.0 was the framing:
  the progressive demuxer rewrote every video packet as MP4/Matroska AVCC and wrapped every AAC
  frame in ADTS, which turns MPEG-TS's Annex-B video and ADTS audio into data no decoder plays
  (`ff::ProgressiveVideo`, `ff::progressive_aac_frame`), and fed a channel's timestamps unrebased
  (`player::engine::first_open_rebases`).
- **The decode path.** H.264 + AAC is an existing Load payload combination in
  `media/src/player/engine.rs`.
- **A no-Plex entry point for the pipeline.** The `/tmp/plxnative-playurl` dev trigger
  (`media/src/player/playurl.rs`) plays an arbitrary URL with a declared codec pair and no library
  item behind it. Phase 0 uses it unchanged.
- **Keys.** CH▲/▼ already arrive as wcodes 300/301 (`docs/remote-keys.md`), and the PIN keypad
  already reads digits. The LIVE button stays the television's (`docs/remote-keys.md` §5).
- **Clock and widgets.** A local-time clock readout, card rows and the focus engine.

What it does not have: any UDP socket (no SSDP), an XML parser (every PMS read is JSON), a text
field in the declarative Settings form, image loading from a non-PMS origin, and any notion of a
playback with no duration.

## Design

### Layer placement (`ci/module-layers.ini`)

| Piece | Layer | Notes |
|---|---|---|
| SSDP search, `discover.json` / `lineup.json` client, XMLTV parser | `data` | uses `net`; names no Plex type. A sibling of the PMS stores, not part of `plex` |
| Guide store (time-windowed, refreshes) | `data` | screens read it like any other store |
| Live playback route | `media` | may name `data`'s channel type; never Plex's decision types |
| Guide grid widget | `ui` | generic time-axis grid; knows nothing about channels |
| Live TV screens, live player overlay | `screens` / `appkit` | per `screens/src/CLAUDE.md` |

Check `docs/module-layers.md` before cutting the modules; the table is the intent, not a ruling.

### Finding Tunarr

1. **SSDP M-SEARCH** for `urn:schemas-upnp-org:device:MediaServer:1` on `239.255.255.250:1900`,
   keep responders whose UDN is Tunarr's fixed one (or whose `device.xml` says `Tunarr`), then read
   `URLBase` → `/discover.json`. No typing, which keeps the README's "no typing in server
   addresses" rule.
2. **Fallback: enter `host:port`** with the on-screen keyboard Search already has, for a network
   that drops multicast. Whether a webOS app process may send and receive multicast at all is
   unverified and must be measured on the set (Phase 1).
3. Persist the chosen origin; re-read `discover.json` at boot and on entering Live TV.

### Channels and guide

- Lineup: `lineup.json`, dropping Tunarr's empty-install placeholder (the entry whose URL path is
  `/setup`).
- Guide: `GET /api/xmltv.xml`, parsed as a stream into `{channel number → [Airing]}` for
  now − 1 h … now + `programmingHours`. Re-fetch on entering Live TV when older than 30 min, and
  on a timer while the guide is on screen; Tunarr's own rebuild cadence makes anything faster
  pointless.
- Parser: a streaming XML reader crate (e.g. `quick-xml`). That is a `Cargo.toml` / `Cargo.lock`
  change, so its PR carries the before/after `make build-bench` table, and the dependency must
  build for the ARM target with the release feature set.
- Artwork is on the Tunarr origin. The app's rule is every image through PMS's
  `/photo/:/transcode` at card size, and the server already fetches absolute URLs there for
  Search's headshots (`docs/pms-api.md`, "every image through `/photo/:/transcode`"). Passing
  Tunarr's artwork URL as `url=` keeps one image path. The cost is that Live TV art then depends
  on the Plex server being up and able to reach Tunarr. The alternative is teaching the image
  loader a second origin. Decide in Phase 3.

### Playback

A **live route** beside the Plex route, not a branch inside it:

- Source: the lineup entry's `URL`. No MDE, no `/decision`, no transcode request (Tunarr already
  transcoded), no `/:/timeline` reports, no scrobble, no watch-state writes of any kind.
- Declaration: `h264` + `aac` from Tunarr's config. Read the actual codecs off the demuxer's
  probe before the Load rather than assuming the default, since a user can configure HEVC or AC-3.
- No duration and no seek: `duration_ns` is unknown, the scrubber is replaced by the airing's
  progress (start → stop from the guide), and seek keys do nothing.
- Channel change: tear down (close the connection first, so Tunarr's session can be reused or
  released), then open the next URL. CH▲/▼, digit entry with a short commit delay, and "last
  channel". Pass a stable `?token=` per channel so a transient reconnect reattaches rather than
  counting as a new viewer.
- End of stream or connection loss: reconnect with backoff and say so on screen. It is never "the
  end of the item", and Up Next does not apply.
- Frame rate: the Load payload declares a frame-rate class for H.264, and a wrong declaration is
  measured judder (`media/src/player/CLAUDE.md`). With Tunarr's frame rate normalised there is one
  rate per channel. Without it, the route must notice a rate change at a programme boundary and
  re-Load.

### Interface

- A **Live TV** entry beside the libraries, shown whenever Live TV has something to show: a Tunarr
  source, a kept virtual channel, or a channel the library can suggest.
- **On Now** shelf: one card per channel showing the current airing and its progress.
- **Guide**: channels down, time across, a fixed channel column and time header, cells sized by
  duration, focus moving by time (a cursor time plus a channel, not a cell index), paging by
  CH▲/▼. A new `ui` widget, and the main frame-rate risk of this plan on a webOS 4.5 GPU.
- **Airing popover**: title, episode, description, time; Watch.
- **Live player overlay**: channel number and name, the current airing and its progress, what's
  next, a LIVE badge; the existing audio/subtitle panels where the stream carries tracks.

## Recommended Tunarr settings

Server-side settings that make the client simpler; document them in the user-facing install notes:

- Transcode config: **Normalize frame rate: on**, so one channel is one frame rate.
- Keep **Deinterlace: on** (the default).
- Video H.264 (default) or HEVC, audio AAC (default) or AC-3. All four are formats this app feeds
  today; nothing else is.
- HDHR settings: **Auto-discovery: on** (the default), so SSDP finds it.

## Phases

**Phase 0 — prove the stream on the set (no code).** On a debug build, with the TV lock held
(`tv-lock` skill), write `/tmp/plxnative-playurl` with
`{"url":"http://192.0.2.20:8000/stream/channels/<uuid>.ts","vcodec":"h264","acodec":"aac"}`
and watch: does FFmpeg open an unbounded TS without trying to seek to its end, does the picture
start, does A/V stay in sync across a programme boundary, and what happens to the frame-rate
declaration when the rate changes (once with normalisation off, once on). Save the event log.
Also capture `discover.json`, `lineup.json` and `xmltv.xml` from a real Tunarr as test fixtures.
*Done when:* a Tunarr channel plays on the television through the existing pipeline, or the log
names exactly what stops it.

**Phase 1 — source and lineup.** SSDP search (measured on the set: can the app multicast?),
manual-entry fallback, `discover.json` / `lineup.json` client, persisted origin. Host tests
against the Phase 0 fixtures.

**Phase 2 — live playback.** The live route, the overlay without guide data (channel name and
number only), CH▲/▼, digits, last channel, reconnect. This is the first usable slice.

**Phase 3 — guide.** XMLTV parser and guide store, On Now shelf, guide grid widget, airing
popover, overlay with airing progress, artwork path.

**Phase 4 — polish.** Favourites and channel ordering held on the television (no server writes),
a "now playing" item on Home, a remembered last channel.

## Verification

- Parsers, the lineup/guide join and the time-window logic: host unit tests on the Phase 0
  fixtures (`make test-crate`, then `make check`).
- Screens: `ui-sim` with a stub Tunarr serving the fixtures. A stub is required anyway; a real
  Tunarr's guide changes every refresh.
- Playback, frame pacing, the guide's frame rate, multicast: the television only (`which-tier`,
  `tv-session`, an `ARM_PROFILE=release` build for any timing claim).
- No step writes to a Plex account. Tunarr is read-only from here too.

## Risks and open questions

1. **Does the pipeline accept an unbounded TS as-is?** Phase 0 answers it. The likely trouble is
   FFmpeg's duration estimation trying to seek on a source that cannot.
2. **Mid-stream frame-rate change** when Tunarr does not normalise: needs a re-Load at the
   boundary or the setting above.
3. **Multicast from a webOS app** is unverified. If it is blocked, manual entry is the only way in,
   and that needs a text field in Settings.
4. **Guide grid performance** at 60 fps on the 2019 set.
5. **Tunarr API drift.** The HDHR and XMLTV surfaces are what Plex and Jellyfin depend on, so they
   are the stable part of Tunarr; avoid its private `/api/*` JSON routes.
6. **Tuner count is advertised, not enforced.** Several televisions on different channels each
   start their own transcode on the Tunarr host; that host's capacity is the real limit.

## Virtual channels

**Status: implemented for 0.12.0; the pages are seen in the simulator against the mock PMS, playback
is not yet verified on the television.** The app makes TV channels out of the viewer's own Plex
library and airs them itself, beside (or instead of) Tunarr's. Code: `plx_data::vchannel` (its
module doc indexes the parts), the lineup merge in `plx_data::livetv` (`virtual_channel`,
`republish`), Home's shelf `plx_data::livetv::suggested`, the channel studio
`plx_screens::livetv::studio` and the tune in `app::livetv` (`tune_virtual`).

- **A channel is a recipe**: where its programmes come from (a playlist, collection, show, season,
  or the library narrowed by rules), the rules, the order style, a seed and a start. The timeline is
  computed from it, deterministically, so every television airs the same thing at the same moment
  (`vchannel::schedule`).
- **Kept channels are Plex playlists** owned by the profile, titled with a TV mark; the recipe rides
  in the playlist's description after `[plxnative-channel v1]` (`vchannel::recipe`), so every
  television in the house reads the same channels. They number from 900 and join the lineup with a
  `plxvc:<playlist>` URL; a Tunarr channel with the same number wins.
- **Suggestions** come from the profile's own view state, the hour, the weekday and the month
  (`vchannel::taste`, `vchannel::suggest`): fourteen families of idea, scored, chosen for variety,
  named and explained. They are shown on Home (Suggested Channels) and in the channel studio; the
  app only suggests and never makes a channel by itself. "Not interested" is remembered per profile
  on the television (`vchannel::dismissed`).
- **The channel studio** (the Live TV page's fourth face) previews a suggestion's live timeline
  before it is kept and offers Keep, Reshuffle, Order, Edit (films and/or shows, unwatched only, a
  rating ceiling, with a live count) and Not interested; on a kept channel, Watch, Reshuffle, Order
  and Delete.
- **Making a channel**: *Make a Channel* on a show, season, collection or playlist card (its hold
  menu) opens the studio on a channel drawn from that title; *New Channel* (at the end of Your
  Channels) builds one from the library by its options — genre, decade, films and/or shows,
  unwatched only, rating — each offering only values that still air something, with a live count
  and a name it takes from its options ("90s Sitcoms"). A library channel's membership follows the
  library: it is rebuilt from its rules whenever the catalog is read again. It opens from the guide's Channels pill, from a Home card, or is the page itself when
  there is no Tunarr server and no kept channel.
- **Watching** plays the library item the timeline airs now, from the moment the channel is at,
  quietly (`route::request_play_channel`: no PlayQueue, timeline or scrobble — a channel never
  touches the profile's history), under the Live TV session, so the banner and the channel keys
  work. The next programme follows when one ends; a pause falls behind live and OK jumps back.
