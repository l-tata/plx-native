//! The progressive demuxer fed an MPEG-TS source — a Live TV channel from Tunarr, whose H.264 or
//! HEVC is Annex-B with its parameter sets in band and whose AAC is already ADTS-framed — next to
//! the MP4/Matroska sources it was written for.

use super::*;

const SPS: [u8; 6] = [0, 0, 0, 1, 0x67, 0x64];
const PPS: [u8; 6] = [0, 0, 0, 1, 0x68, 0xee];
const AUD: [u8; 6] = [0, 0, 0, 1, 0x09, 0x10];

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// **The Live TV stall.** Tunarr's first H.264 access unit is `AUD SPS PPS IDR` in Annex-B, and
/// FFmpeg's MPEG-TS demuxer leaves the stream's extradata as those same Annex-B parameter sets.
/// The AU must reach the decoder whole; reading its first start code as an AVCC length of one fed
/// the decoder a single byte per frame and the channel never left Buffering.
#[test]
fn a_tunarr_h264_keyframe_reaches_the_decoder_whole() {
    let extradata = cat(&[&SPS, &PPS]);
    let idr = cat(&[&AUD, &SPS, &PPS, &[0, 0, 1, 0x65, 0x88, 0x84, 0x21]]);
    let mut video = ProgressiveVideo::new(&extradata, false);
    let mut out = Vec::new();
    assert_eq!(video.access_unit(&idr, &mut out), Ok(true));
    assert_eq!(out, idr);
    let p = cat(&[&AUD, &[0, 0, 1, 0x41, 0x9a, 0x02]]);
    assert_eq!(video.access_unit(&p, &mut out), Ok(false));
    assert_eq!(out, p, "a P frame passes through untouched");
}

/// No extradata at all: the first packet's 4-byte start code is what says Annex-B (a NAL of one
/// byte, which is what an AVCC reading of `00 00 00 01` would mean, does not exist).
#[test]
fn without_extradata_the_first_packet_decides_the_framing() {
    let mut video = ProgressiveVideo::new(&[], false);
    let idr = cat(&[&SPS, &PPS, &[0, 0, 0, 1, 0x65, 1, 2]]);
    let mut out = Vec::new();
    assert_eq!(video.access_unit(&idr, &mut out), Ok(true));
    assert_eq!(out, idr);
}

/// An IDR that arrives without its parameter sets gets the cached ones in front — from the
/// extradata before any packet carried them.
#[test]
fn an_idr_without_in_band_parameter_sets_gets_the_cached_ones() {
    let mut video = ProgressiveVideo::new(&cat(&[&SPS, &PPS]), false);
    let idr = [0, 0, 0, 1, 0x65, 7, 7];
    let mut out = Vec::new();
    assert_eq!(video.access_unit(&idr, &mut out), Ok(true));
    assert_eq!(out, cat(&[&SPS, &PPS, &idr]));
}

/// Joining a channel mid-GOP with no parameter set seen yet: that IDR cannot be decoded and is
/// refused (the demuxer drops it); the next one carrying its sets plays.
#[test]
fn an_idr_with_no_parameter_sets_anywhere_is_refused_until_one_arrives() {
    let mut video = ProgressiveVideo::new(&[], false);
    let mut out = Vec::new();
    assert!(video.access_unit(&[0, 0, 0, 1, 0x65, 1], &mut out).is_err());
    let idr = cat(&[&SPS, &PPS, &[0, 0, 0, 1, 0x65, 2]]);
    assert_eq!(video.access_unit(&idr, &mut out), Ok(true));
    assert_eq!(out, idr);
}

/// HEVC in MPEG-TS: VPS/SPS/PPS (types 32/33/34) are kept from the stream and put in front of an
/// IRAP (16..=23) that lacks them.
#[test]
fn a_tunarr_hevc_keyframe_gets_its_vps_sps_pps() {
    let vps = [0, 0, 0, 1, 0x40, 0x01, 0xaa];
    let sps = [0, 0, 0, 1, 0x42, 0x01, 0xbb];
    let pps = [0, 0, 0, 1, 0x44, 0x01, 0xcc];
    let first = cat(&[&vps, &sps, &pps, &[0, 0, 0, 1, 0x26, 0x01, 1]]); // IDR_W_RADL (19)
    let mut video = ProgressiveVideo::new(&[], true);
    let mut out = Vec::new();
    assert_eq!(video.access_unit(&first, &mut out), Ok(true));
    assert_eq!(out, first);
    let later = [0, 0, 0, 1, 0x28, 0x01, 2]; // CRA (20)
    assert_eq!(video.access_unit(&later, &mut out), Ok(true));
    assert_eq!(out, cat(&[&vps, &sps, &pps, &later]));
    let trail = [0, 0, 0, 1, 0x02, 0x01, 3]; // TRAIL_R (1)
    assert_eq!(video.access_unit(&trail, &mut out), Ok(false));
}

/// The MP4/Matroska path is unchanged: an avcC record means length-prefixed NALs, rewritten to
/// Annex-B with the record's parameter sets in front of the IDR.
#[test]
fn an_avcc_source_is_still_rewritten_from_length_prefixes() {
    // avcC: version 1, profile/compat/level, 0xff (4-byte lengths), 1 SPS, 1 PPS.
    let avcc = [1, 0x64, 0, 0x1f, 0xff, 0xe1, 0, 2, 0x67, 0x64, 1, 0, 2, 0x68, 0xee];
    let mut video = ProgressiveVideo::new(&avcc, false);
    let mut out = Vec::new();
    assert_eq!(video.access_unit(&[0, 0, 0, 3, 0x65, 9, 9], &mut out), Ok(true));
    assert_eq!(out, [0, 0, 0, 1, 0x67, 0x64, 0, 0, 0, 1, 0x68, 0xee, 0, 0, 0, 1, 0x65, 9, 9]);
}

/// MPEG-TS AAC is already ADTS; a second header in front of it is a frame LG's decoder cannot
/// parse, and with audioSync a dead audio clock holds the video too.
#[test]
fn an_adts_frame_from_mpeg_ts_is_not_wrapped_twice() {
    let mut adts = adts_header(4, 2, 5).to_vec();
    adts.extend_from_slice(&[0x21, 0x10, 0x05, 0x00, 0xa0]);
    assert_eq!(progressive_aac_frame(&adts, 4, 2), adts);
    let raw = [0x21, 0x10, 0x05, 0x00, 0xa0];
    let framed = progressive_aac_frame(&raw, 4, 2);
    assert_eq!(&framed[..7], &adts_header(4, 2, 5));
    assert_eq!(&framed[7..], &raw);
}

/// Tunarr's MPEG-TS packs several AAC frames into one PES, and the PES timestamp belongs to the
/// first frame that starts in it. libavformat stamps the others from the frame duration once its
/// own running clock is set, but not before it; the progressive demuxer used to feed such a frame
/// at time 0. A frame without a timestamp starts where the one before it ended.
#[test]
fn an_aac_frame_without_a_timestamp_starts_where_the_last_one_ended() {
    const FRAME: i64 = 21_333_333; // 1024 samples at 48 kHz
    let mut clock = ProgressiveAudioClock::default();
    assert_eq!(clock.stamp(Some(900_000_000), Some(FRAME)), Some(900_000_000));
    assert_eq!(clock.stamp(None, Some(FRAME)), Some(900_000_000 + FRAME));
    assert_eq!(clock.stamp(None, Some(FRAME)), Some(900_000_000 + 2 * FRAME));
    // The next PES carries its own timestamp again, and it wins.
    assert_eq!(clock.stamp(Some(1_000_000_000), Some(FRAME)), Some(1_000_000_000));
    assert_eq!(clock.stamp(None, Some(FRAME)), Some(1_000_000_000 + FRAME));
}

/// Joined mid-PES, the first frames have nothing to count from: they are dropped rather than fed
/// at 0. After a seek the count starts again from the next timestamp.
#[test]
fn an_unanchored_aac_frame_is_dropped_and_a_seek_forgets_the_anchor() {
    let mut clock = ProgressiveAudioClock::default();
    assert_eq!(clock.stamp(None, Some(20_000_000)), None);
    assert_eq!(clock.stamp(Some(5_000_000_000), Some(20_000_000)), Some(5_000_000_000));
    clock.reset();
    assert_eq!(clock.stamp(None, Some(20_000_000)), None);
    // A frame of unknown length leaves the next one with nothing to count from.
    assert_eq!(clock.stamp(Some(7_000_000_000), None), Some(7_000_000_000));
    assert_eq!(clock.stamp(None, Some(20_000_000)), None);
}

/// The frame length comes from the packet when FFmpeg knows it, and from the ADTS header when it
/// does not (1024 samples per raw data block, at the header's own rate).
#[test]
fn an_aac_frame_length_is_read_from_the_packet_or_its_adts_header() {
    let mut adts = adts_header(3, 2, 5).to_vec(); // 48 kHz
    adts.extend_from_slice(&[0x21, 0x10, 0x05, 0x00, 0xa0]);
    assert_eq!(audio_frame_ns(Some(23_000_000), &adts), Some(23_000_000));
    assert_eq!(audio_frame_ns(None, &adts), Some(21_333_333));
    assert_eq!(audio_frame_ns(None, &[0x0b, 0x77, 0, 0, 0, 0, 0]), None);
}
