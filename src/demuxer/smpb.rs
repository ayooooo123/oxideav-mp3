// SPDX-License-Identifier: LGPL-2.1-or-later
// Port of FFmpeg 2da55bf libavformat/mp3dec.c (find_itunes_smpb) and
// libavformat/demux_utils.c (ff_itunes_parse_smpb), with the parts of
// libavformat/id3v2.c that decide which COMM or TXXX frame those see: every
// tag at the start of the file, read_lang_descr_tag's comment keys, and the
// first frame of a key winning. oxideav-id3 decodes the frames.
// Copyright (c) 2003 Fabrice Bellard (mp3dec.c, id3v2.c);
// 2000-2002 Fabrice Bellard (demux_utils.c)
//
// This file is free software; you can redistribute it and/or modify it under
// the terms of the GNU Lesser General Public License as published by the Free
// Software Foundation; either version 2.1, or (at your option) any later version.
// It is distributed WITHOUT ANY WARRANTY; without even the implied warranty
// of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See LICENSE-LGPL.

//! iTunSMPB gapless counts in an MP3's ID3v2 tags, found as FFmpeg finds
//! them: the first COMM frame described `iTunSMPB` (in any case) whose
//! language is `und`, `xxx`, empty or three characters long, else the first
//! TXXX frame with that description. A frame FFmpeg would find but cannot
//! parse gives nothing; no other frame stands in for it. Only those two
//! frame types are read; every other frame is sought past.

use std::io::{Cursor, SeekFrom};

use oxideav_core::{ReadSeek, Result};
use oxideav_id3::Id3Frame;

/// The most a tag unsynchronised in memory, or one COMM/TXXX frame, may
/// hold. Larger metadata is ignored, not an audio failure; oxideav-id3
/// caps a compressed frame's output at 64 MiB itself.
const MAX_TAG_BYTES: usize = 16 << 20;
/// Consecutive tags read at the start of the file (FFmpeg reads all).
const MAX_TAGS: usize = 16;

type Counts = (i64, i64, i64);

/// The iTunSMPB `(priming, remainder, samples)` of the ID3v2 tags at the
/// start of `input`. Damaged or truncated metadata yields `None`.
pub(super) fn read(input: &mut dyn ReadSeek) -> Option<Counts> {
    let mut found = Found::default();
    let mut start = 0u64;
    for _ in 0..MAX_TAGS {
        let Ok(Some(length)) = tag(input, start, &mut found) else { break };
        start += length;
    }
    found.comment.or(found.user).flatten()
}

/// The frames FFmpeg's lookup settles on, parsed or not.
#[derive(Default)]
struct Found {
    comment: Option<Option<Counts>>,
    user: Option<Option<Counts>>,
}

fn number(bytes: &[u8], synchsafe: bool) -> Option<usize> {
    bytes.iter().try_fold(0usize, |n, &b| {
        if synchsafe && b > 127 {
            return None;
        }
        n.checked_mul(if synchsafe { 128 } else { 256 })?.checked_add(b as usize)
    })
}

/// `ff_itunes_parse_smpb`: four `%16x` fields (at most 16 characters each,
/// counting a sign and a `0x`), none cut short before another hex digit,
/// none of the counts above 2^40.
fn counts(text: &str) -> Option<Counts> {
    let b = text.as_bytes();
    let mut at = 0;
    let mut values = [0u64; 4];
    for value in &mut values {
        while b.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        let end = b.len().min(at + 16);
        let mut p = at;
        let negative = b.get(p) == Some(&b'-');
        if p < end && matches!(b[p], b'+' | b'-') {
            p += 1;
        }
        if p + 2 < end && b[p] == b'0' && (b[p + 1] | 0x20) == b'x' && b[p + 2].is_ascii_hexdigit() {
            p += 2;
        }
        let digits = p;
        let mut v = 0u64;
        while p < end && b[p].is_ascii_hexdigit() {
            v = (v << 4) | u64::from(char::from(b[p]).to_digit(16)?);
            p += 1;
        }
        if p == digits || b.get(p).is_some_and(u8::is_ascii_hexdigit) {
            return None;
        }
        *value = if negative { v.wrapping_neg() } else { v };
        at = p;
    }
    let [_, priming, remainder, samples] = values;
    if [priming, remainder, samples].iter().any(|&n| n > 1 << 40) {
        return None;
    }
    Some((priming as i64, remainder as i64, samples as i64))
}

/// FFmpeg keeps a COMM frame under "comment-<description>[-<language>]";
/// `find_itunes_smpb` takes `und`/`xxx`/empty, or exactly three characters.
fn language_ok(lang: [u8; 3]) -> bool {
    let lower = lang.map(|c| c.to_ascii_lowercase());
    let len = lower.iter().position(|&c| c == 0).unwrap_or(3);
    &lower == b"und" || &lower == b"xxx" || len == 0 || len == 3
}

/// Reads the tag at `start` into `found`; its length when one is there.
/// Versions FFmpeg skips (not 2 to 4, compressed 2.2) are skipped whole.
fn tag(input: &mut dyn ReadSeek, start: u64, found: &mut Found) -> Result<Option<u64>> {
    input.seek(SeekFrom::Start(start))?;
    let mut header = [0u8; 10];
    input.read_exact(&mut header)?;
    // ff_id3v2_match.
    if &header[..3] != b"ID3" || header[3] == 0xff || header[4] == 0xff {
        return Ok(None);
    }
    let Some(size) = number(&header[6..10], true) else { return Ok(None) };
    let (version, flags) = (header[3], header[5]);
    let length = 10 + size as u64 + if version == 4 && flags & 0x10 != 0 { 10 } else { 0 };
    if !matches!(version, 2..=4) || version == 2 && flags & 0x40 != 0 {
        return Ok(Some(length));
    }
    let mut unsynchronised;
    let (input, mut at, end): (&mut dyn ReadSeek, u64, u64) = if version < 4 && flags & 0x80 != 0 {
        if size > MAX_TAG_BYTES {
            return Ok(Some(length));
        }
        let mut body = vec![0u8; size];
        input.read_exact(&mut body)?;
        let mut after_ff = false;
        body.retain(|&b| {
            let keep = !(after_ff && b == 0);
            after_ff = b == 255;
            keep
        });
        let end = body.len() as u64;
        unsynchronised = Cursor::new(body);
        (&mut unsynchronised, 0, end)
    } else {
        (input, start + 10, start + 10 + size as u64)
    };
    if version >= 3 && flags & 0x40 != 0 {
        if end - at < 4 {
            return Ok(Some(length));
        }
        let mut extended = [0u8; 4];
        input.read_exact(&mut extended)?;
        let Some(n) = number(&extended, version == 4) else { return Ok(Some(length)) };
        at += n as u64 + if version == 3 { 4 } else { 0 };
        if n < 6 || at > end {
            return Ok(Some(length));
        }
    }
    let header_len = if version == 2 { 6 } else { 10 };
    while end.saturating_sub(at) >= header_len as u64 {
        input.seek(SeekFrom::Start(at))?;
        let mut frame = [0u8; 10];
        input.read_exact(&mut frame[..header_len])?;
        if frame[0] == 0 {
            break;
        }
        let (id, size) = if version == 2 {
            (&frame[..3], number(&frame[3..6], false))
        } else {
            (&frame[..4], number(&frame[4..8], version == 4))
        };
        let Some(size) = size else { break };
        at += header_len as u64;
        if size as u64 > end - at {
            break;
        }
        at += size as u64;
        let comment = matches!(id, b"COMM" | b"COM");
        let wanted = if comment { found.comment.is_none() } else { matches!(id, b"TXXX" | b"TXX") && found.user.is_none() };
        if !wanted || size > MAX_TAG_BYTES {
            continue;
        }
        // One selected frame at a time, as a tag of its own: oxideav-id3
        // handles text encodings, grouping, compression and v2.4
        // unsynchronisation.
        let n = header_len + size;
        let mut single = Vec::with_capacity(10 + n);
        single.extend_from_slice(b"ID3");
        single.extend_from_slice(&[version, 0, if version == 4 { flags & 0x80 } else { 0 }]);
        single.extend_from_slice(&[((n >> 21) & 127) as u8, ((n >> 14) & 127) as u8, ((n >> 7) & 127) as u8, (n & 127) as u8]);
        single.extend_from_slice(&frame[..header_len]);
        single.resize(10 + n, 0);
        input.read_exact(&mut single[10 + header_len..])?;
        let Ok((parsed, _)) = oxideav_id3::parse_tag(&single) else { continue };
        for frame in parsed.frames {
            match frame {
                Id3Frame::Comment { lang, description, text }
                    if found.comment.is_none() && description.eq_ignore_ascii_case("iTunSMPB") && language_ok(lang) =>
                {
                    found.comment = Some(counts(&text));
                }
                Id3Frame::UserText { description, value }
                    if found.user.is_none() && description.eq_ignore_ascii_case("iTunSMPB") =>
                {
                    found.user = Some(counts(&value));
                }
                _ => {}
            }
        }
    }
    Ok(Some(length))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_read_as_ffmpegs_sscanf_does() {
        let itunes = " 00000000 00000210 0000086A 0000000000066486 00000000 0002DA9D";
        assert_eq!(counts(itunes), Some((0x210, 0x86A, 0x66486)));
        // A sign and a 0x count toward the 16 characters; a field may end at
        // any character but a hex digit, and fields need no space before a sign.
        assert_eq!(counts("0 +0x210 86A 66486xyz"), Some((0x210, 0x86A, 0x66486)));
        assert_eq!(counts("0+210+86A+66486"), Some((0x210, 0x86A, 0x66486)));
        assert_eq!(counts("0 210 86A 0000000000066486F"), None, "cut short at 16 digits");
        assert_eq!(counts("0 +0x00000000000210 86A 1"), None, "cut short by its 0x");
        assert_eq!(counts("0,210,86A,1"), None);
        assert_eq!(counts("0 210 86A"), None);
        assert_eq!(counts("0 -1 86A 1"), None, "negative wraps past 2^40");
        assert_eq!(counts("0 -0 86A 1"), Some((0, 0x86A, 1)));
        assert_eq!(counts("0 210 86A 10000000001"), None, "above 2^40");
    }

    #[test]
    fn comment_languages_follow_ffmpegs_keys() {
        for ok in [*b"eng", *b"und", *b"XXX", *b"123", [0, 0, 0], [0, b'e', b'n']] {
            assert!(language_ok(ok), "{ok:?}");
        }
        for bad in [[b'e', 0, 0], [b'e', b'n', 0]] {
            assert!(!language_ok(bad), "{bad:?}");
        }
    }
}
