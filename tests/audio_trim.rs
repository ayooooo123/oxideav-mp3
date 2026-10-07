//! Gapless MP3 as FFmpeg 2da55bf's mp3 demuxer reads it:
//! `Demuxer::packet_metadata().audio_trim` skips a LAME / Lavf / Lavc info
//! tag's encoder delay plus the decoder's 529 samples from the first audio
//! frame, and drops the padding the frame count places at the end. The info
//! tag is read after whichever Xing fields its flags select.

use std::io::Cursor;

use oxideav_core::{AudioTrim, Demuxer, Error};
use oxideav_mp3::demuxer::Mp3Demuxer;

/// MPEG-1 Layer III, 128 kbit/s, 44.1 kHz, stereo, no CRC: 417 bytes.
const HEADER: [u8; 4] = [0xFF, 0xFB, 0x90, 0x00];
const FRAME_LEN: usize = 417;

/// An info frame (`Xing` fields per `flags`) with `encoder` and the
/// 12-bit delay / padding pair, then `frames` silent audio frames.
fn mp3(flags: u32, encoder: &[u8; 9], delay: u32, padding: u32, frames: u32) -> Vec<u8> {
    let mut info = HEADER.to_vec();
    info.resize(4 + 32, 0);
    info.extend_from_slice(b"Info");
    info.extend_from_slice(&flags.to_be_bytes());
    if flags & 1 != 0 {
        info.extend_from_slice(&frames.to_be_bytes());
    }
    if flags & 2 != 0 {
        info.extend_from_slice(&((frames + 1) * FRAME_LEN as u32).to_be_bytes());
    }
    if flags & 4 != 0 {
        info.extend((0..100u8).map(|i| (i as u32 * 255 / 99) as u8));
    }
    if flags & 8 != 0 {
        info.extend_from_slice(&0u32.to_be_bytes());
    }
    info.extend_from_slice(encoder);
    info.extend_from_slice(&[0; 12]);
    info.extend_from_slice(&((delay << 12) | padding).to_be_bytes()[1..]);
    info.resize(FRAME_LEN, 0);
    let mut audio = HEADER.to_vec();
    audio.resize(FRAME_LEN, 0);
    [info, audio.repeat(frames as usize)].concat()
}

fn open(bytes: Vec<u8>) -> Box<dyn Demuxer> {
    Box::new(Mp3Demuxer::open(Box::new(Cursor::new(bytes))).unwrap())
}

fn trims(d: &mut dyn Demuxer) -> Vec<Option<AudioTrim>> {
    let mut out = Vec::new();
    loop {
        match d.next_packet() {
            Ok(_) => out.push(d.packet_metadata().audio_trim),
            Err(Error::Eof) => return out,
            Err(e) => panic!("demux: {e}"),
        }
    }
}

fn trim(skip: u32, discard: u32) -> Option<AudioTrim> {
    Some(AudioTrim { skip_samples: skip, discard_padding: discard, sample_rate: 44_100 })
}

/// FFmpeg's figures for FATE `gapless/gapless.mp3` (delay 576, padding
/// 1984): 1105 samples skipped, 303 + 1152 dropped from the last two frames.
fn lame_expected() -> Vec<Option<AudioTrim>> {
    let mut expected = vec![None; 10];
    expected[0] = trim(576 + 529, 0);
    expected[8] = trim(0, 303);
    expected[9] = trim(0, 1152);
    expected
}

#[test]
fn lame_delay_and_padding_trim_the_first_and_last_frames() {
    assert_eq!(trims(&mut *open(mp3(0x0F, b"LAME3.100", 576, 1984, 10))), lame_expected());
}

#[test]
fn the_info_tag_follows_whichever_xing_fields_are_present() {
    // Frame count only, and a Lavc-written tag.
    assert_eq!(trims(&mut *open(mp3(0x01, b"Lavc61.19", 576, 1984, 10))), lame_expected());
}

#[test]
fn other_encoders_and_missing_frame_counts_trim_what_is_known() {
    assert_eq!(trims(&mut *open(mp3(0x0F, b"GOGO3.13\0", 576, 1984, 10))), vec![None; 10]);
    // Without a frame count only the start is known.
    let mut expected = vec![None; 10];
    expected[0] = trim(1105, 0);
    assert_eq!(trims(&mut *open(mp3(0x0C, b"LAME3.100", 576, 1984, 10))), expected);
}

#[test]
fn a_seek_back_to_the_start_skips_the_delay_again() {
    let mut d = open(mp3(0x0F, b"LAME3.100", 576, 1984, 10));
    d.next_packet().unwrap();
    assert_eq!(d.packet_metadata().audio_trim, trim(1105, 0));
    assert_eq!(d.seek_to(0, 0).unwrap(), 0);
    assert_eq!(d.packet_metadata().audio_trim, None, "cleared by the seek");
    assert_eq!(trims(&mut *d), lame_expected());
}

/// An ID3v2.3 COMM (with `lang`) or TXXX frame described iTunSMPB.
fn itunes_frame(kind: &[u8; 4], lang: &[u8; 3], value: &str) -> Vec<u8> {
    let mut body = vec![0];
    if kind == b"COMM" { body.extend_from_slice(lang); }
    body.extend_from_slice(b"iTunSMPB\0");
    body.extend_from_slice(value.as_bytes());
    let mut frame = kind.to_vec();
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&[0, 0]);
    frame.extend(body);
    frame
}

/// An ID3v2.3 tag holding `frames`.
fn id3(frames: &[Vec<u8>]) -> Vec<u8> {
    let body = frames.concat();
    let n = body.len() as u32;
    let mut tag = b"ID3\x03\0\0".to_vec();
    tag.extend_from_slice(&[((n >> 21) & 127) as u8, ((n >> 14) & 127) as u8, ((n >> 7) & 127) as u8, (n & 127) as u8]);
    tag.extend(body);
    tag
}

fn itunes_tag(kind: &[u8; 4], lang: &[u8; 3], value: &str) -> Vec<u8> {
    id3(&[itunes_frame(kind, lang, value)])
}

#[test]
fn itunes_comment_and_user_text_include_decoder_delay_once() {
    // The same start/end counts as FATE gapless-itunes.mp3, on 10 frames.
    let value = " 00000000 00000210 0000086A 0000000000002286";
    for (kind, lang) in [(b"COMM", b"eng"), (b"COMM", b"fra"), (b"COMM", b"und"), (b"TXXX", b"eng")] {
        let bytes = [itunes_tag(kind, lang, value), mp3(1, b"iTunes9.0", 0, 0, 10)].concat();
        let mut expected = vec![None; 10];
        expected[0] = trim(528, 0);
        expected[8] = trim(0, 1002);
        expected[9] = trim(0, 1152);
        let mut d = open(bytes);
        assert_eq!(d.streams()[0].duration, Some(8838));
        assert_eq!(trims(&mut *d), expected);
    }
}

#[test]
fn itunes_counts_are_checked_and_never_replace_lame() {
    let good = "00000000 00000210 0000086A 0000000000002286";
    let bytes = [itunes_tag(b"COMM", b"eng", good), mp3(0x0F, b"LAME3.100", 576, 1984, 10)].concat();
    assert_eq!(trims(&mut *open(bytes)), lame_expected());
    for bad in [
        "0 4001 1 1",             // Priming above 16384.
        "0 210 86A 0",            // No presented samples.
        "0 210 86A 2287",         // Counts disagree with Xing.
        "0 210 86A 10000000001",  // Count above 2^40.
        "0 210 86A 00000000000000001", // A truncated 17-digit field.
    ] {
        let bytes = [itunes_tag(b"COMM", b"eng", bad), mp3(1, b"iTunes9.0", 0, 0, 10)].concat();
        assert_eq!(trims(&mut *open(bytes)), vec![None; 10], "{bad}");
    }
    // Without a Xing frame count, a valid iTunes window still applies.
    let bytes = [itunes_tag(b"TXXX", b"eng", good), mp3(0, b"iTunes9.0", 0, 0, 10)].concat();
    let got = trims(&mut *open(bytes));
    assert_eq!(got[0], trim(528, 0));
    assert_eq!(got[8..], [trim(0, 1002), trim(0, 1152)]);
}

#[test]
fn the_frame_ffmpeg_looks_up_decides() {
    let good = "0 210 86A 2286";
    let bad = "0 4001 1 1"; // Parses, but primes past 16384 samples.
    let start = |tags: Vec<u8>| trims(&mut *open([tags, mp3(1, b"iTunes9.0", 0, 0, 10)].concat()))[0];
    // A COMM wins over an earlier TXXX; the first TXXX over later ones.
    assert_eq!(start(id3(&[itunes_frame(b"TXXX", b"eng", bad), itunes_frame(b"COMM", b"eng", good)])), trim(528, 0));
    assert_eq!(start(id3(&[itunes_frame(b"TXXX", b"eng", good), itunes_frame(b"TXXX", b"eng", bad)])), trim(528, 0));
    assert_eq!(start(id3(&[itunes_frame(b"TXXX", b"eng", bad), itunes_frame(b"TXXX", b"eng", good)])), None);
    // FFmpeg keys a COMM with a one- or two-letter language where it does
    // not look: the TXXX decides.
    assert_eq!(start(id3(&[itunes_frame(b"COMM", b"e\0\0", good)])), None);
    assert_eq!(start(id3(&[itunes_frame(b"COMM", b"en\0", bad), itunes_frame(b"TXXX", b"eng", good)])), trim(528, 0));
    // The first COMM settles it, parsed or not.
    assert_eq!(start(id3(&[itunes_frame(b"COMM", b"eng", "junk"), itunes_frame(b"COMM", b"fra", good)])), None);
    // Every tag at the start counts, as FFmpeg reads them all.
    assert_eq!(start([id3(&[]), itunes_tag(b"COMM", b"eng", good)].concat()), trim(528, 0));
}
