//! Layer I and II streams the mp3 demuxer reads are named `mp1` and `mp2`,
//! as FFmpeg 2da55bf's MPEG audio parser names the streams its mp3 demuxer
//! reads, and their bare frame syncs probe as MPEG audio once four frames
//! follow one another (a raw `.mpa` file has no other signature).

use std::io::Cursor;

use oxideav_core::{CodecTag, Demuxer, ProbeData};
use oxideav_mp3::demuxer::{Mp3Demuxer, probe};

/// `frames` silent MPEG-1 frames at 32 kHz, mono, 32 kbit/s: every bit
/// allocation zero. Layer I frames are 48 bytes, Layer II 144.
fn stream(layer_bits: u8, frames: usize) -> Vec<u8> {
    let (second, len) = match layer_bits {
        0b11 => (0xFF, 48),
        _ => (0xFD, 144),
    };
    let mut out = Vec::new();
    for _ in 0..frames {
        let start = out.len();
        out.extend_from_slice(&[0xFF, second, 0x18, 0xC0]);
        out.resize(start + len, 0);
    }
    out
}

fn open(bytes: Vec<u8>) -> Mp3Demuxer {
    Mp3Demuxer::open(Box::new(Cursor::new(bytes))).expect("open")
}

#[test]
fn layer_one_is_mp1() {
    let mut d = open(stream(0b11, 6));
    let p = &d.streams()[0].params;
    assert_eq!(p.codec_id.as_str(), "mp1");
    assert_eq!(p.tag, Some(CodecTag::wave_format(0x0050)));
    assert_eq!((p.sample_rate, p.channels), (Some(32000), Some(1)));
    let pts: Vec<_> = std::iter::from_fn(|| d.next_packet().ok()).map(|p| p.pts.unwrap()).collect();
    assert_eq!(pts, [0, 384, 768, 1152, 1536, 1920]);
}

#[test]
fn layer_two_is_mp2() {
    let d = open(stream(0b10, 6));
    assert_eq!(d.streams()[0].params.codec_id.as_str(), "mp2");
}

#[test]
fn raw_layer_one_and_two_probe_with_four_frames() {
    for layer in [0b11, 0b10] {
        let four = stream(layer, 4);
        assert_eq!(probe(&ProbeData { buf: &four, ext: None }), 75, "layer bits {layer:02b}");
        assert_eq!(probe(&ProbeData { buf: &four, ext: Some("mpa") }), 100);
        let three = stream(layer, 3);
        assert_eq!(probe(&ProbeData { buf: &three, ext: None }), 0, "three frames are not enough");
    }
}
