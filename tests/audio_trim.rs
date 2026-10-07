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
    trims(&mut *d);
    assert_eq!(d.seek_to(0, 0).unwrap(), 0);
    assert_eq!(d.packet_metadata().audio_trim, None, "cleared by the seek");
    assert_eq!(trims(&mut *d), lame_expected());
}
