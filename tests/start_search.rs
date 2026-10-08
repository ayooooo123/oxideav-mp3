//! Where audio starts, as FFmpeg 2da55bf's mp3_read_header finds it: at the
//! first frame whose next frame's header agrees with its own under
//! MP3_MASK (sync, version, layer, rate, channel mode, copyright, original,
//! emphasis). The ISO Layer I vector in FFmpeg's sample archive starts with
//! a stereo frame before joint stereo ones, and FFmpeg skips it as junk
//! ("Skipping 576 bytes of junk at 0").

use std::io::Cursor;

use oxideav_core::Demuxer;
use oxideav_mp3::demuxer::Mp3Demuxer;

/// A 48-byte MPEG-1 Layer I frame (32 kHz, 32 kbit/s) in channel `mode`
/// (0 stereo, 1 joint stereo, 3 mono), its last byte `tag`.
fn frame(mode: u8, tag: u8) -> Vec<u8> {
    let mut f = vec![0xFF, 0xFF, 0x18, mode << 6];
    f.resize(48, 0);
    f[47] = tag;
    f
}

fn open(frames: &[(u8, u8)]) -> Mp3Demuxer {
    let data: Vec<u8> = frames.iter().flat_map(|&(m, t)| frame(m, t)).collect();
    Mp3Demuxer::open(Box::new(Cursor::new(data))).expect("open")
}

fn tags(d: &mut Mp3Demuxer) -> Vec<(i64, u8)> {
    std::iter::from_fn(|| d.next_packet().ok()).map(|p| (p.pts.unwrap(), p.data[47])).collect()
}

#[test]
fn a_first_frame_in_another_mode_is_junk() {
    let mut d = open(&[(0, 0), (1, 1), (1, 2), (1, 3), (1, 4)]);
    assert_eq!(tags(&mut d), [(0, 1), (384, 2), (768, 3), (1152, 4)]);
}

#[test]
fn the_stream_takes_the_channels_of_the_frame_audio_starts_at() {
    let mut d = open(&[(3, 0), (0, 1), (0, 2), (0, 3), (0, 4)]);
    assert_eq!(d.streams()[0].params.channels, Some(2));
    assert_eq!(tags(&mut d)[0], (0, 1));
}

#[test]
fn agreeing_frames_start_at_the_first() {
    let mut d = open(&[(1, 0), (1, 1), (1, 2), (1, 3)]);
    assert_eq!(tags(&mut d), [(0, 0), (384, 1), (768, 2), (1152, 3)]);
}
