//! A frame the end of the file cuts short is the last packet, holding the
//! bytes that are there: FFmpeg 2da55bf's MPEG audio parser hands a cut
//! frame on when its input ends, and its decoders decode it over zeros
//! (one more frame of output than the whole frames give).

use std::io::Cursor;

use oxideav_core::Demuxer;
use oxideav_mp3::demuxer::Mp3Demuxer;

/// `frames` silent MPEG-1 Layer I frames, 32 kHz mono 32 kbit/s, 48 bytes
/// each.
fn layer1(frames: usize) -> Vec<u8> {
    let mut out = Vec::new();
    for _ in 0..frames {
        let start = out.len();
        out.extend_from_slice(&[0xFF, 0xFF, 0x18, 0xC0]);
        out.resize(start + 48, 0);
    }
    out
}

#[test]
fn a_cut_last_frame_is_a_packet() {
    let mut data = layer1(6);
    data.truncate(data.len() - 20);
    let mut d = Mp3Demuxer::open(Box::new(Cursor::new(data))).expect("open");
    let packets: Vec<_> = std::iter::from_fn(|| d.next_packet().ok()).map(|p| (p.pts.unwrap(), p.data.len())).collect();
    assert_eq!(packets, [(0, 48), (384, 48), (768, 48), (1152, 48), (1536, 48), (1920, 28)]);
}

#[test]
fn a_whole_last_frame_is_unchanged() {
    let mut d = Mp3Demuxer::open(Box::new(Cursor::new(layer1(6)))).expect("open");
    assert_eq!(std::iter::from_fn(|| d.next_packet().ok()).count(), 6);
}
