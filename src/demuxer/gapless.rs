// SPDX-License-Identifier: LGPL-2.1-or-later
// Port of FFmpeg 2da55bf libavformat/mp3dec.c (the gapless fields of
// mp3_parse_info_tag, mp3_parse_itunes_smpb) and libavformat/demux.c (the
// start skip and the discard window of read_frame_internal).
// Copyright (c) 2003 Fabrice Bellard (mp3dec.c); 2000-2002 Fabrice Bellard
// (demux.c)
//
// This file is free software; you can redistribute it and/or modify it under
// the terms of the GNU Lesser General Public License as published by the Free
// Software Foundation; either version 2.1, or (at your option) any later version.
// It is distributed WITHOUT ANY WARRANTY; without even the implied warranty
// of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See LICENSE-LGPL.

//! Encoder delay and padding of a Layer III stream, as FFmpeg reads them
//! from the info frame's LAME fields or, without those, from iTunSMPB.

use oxideav_core::AudioTrim;

use crate::frame::{Layer, Mp3FrameHeader, MpegVersion};

/// Encoder delay and padding, in decoder output samples counted from the
/// first audio frame after the info frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Gapless {
    /// Start skip, including the decoder's delay.
    start_skip: u32,
    /// The output samples `[first, last)` that are padding, when known.
    window: Option<(i64, i64)>,
    samples_per_frame: i64,
    /// The presented length iTunSMPB states; the LAME fields leave the
    /// duration estimate as it is.
    presented_samples: Option<i64>,
}

impl Gapless {
    /// The trim of the packet whose output starts at sample `pts`
    /// (FFmpeg demux.c `read_frame_internal`).
    pub(super) fn trim_at(&self, pts: i64, sample_rate: u32) -> Option<AudioTrim> {
        let skip = if pts == 0 { self.start_skip } else { 0 };
        let discard = match self.window {
            Some((first, last)) => {
                let end = pts.saturating_add(self.samples_per_frame);
                if first != 0 && end >= first && pts < last {
                    (end - first).min(self.samples_per_frame)
                } else {
                    0
                }
            }
            None => 0,
        };
        (skip > 0 || discard > 0).then(|| AudioTrim {
            skip_samples: skip,
            discard_padding: discard as u32,
            sample_rate,
        })
    }

    /// The presented length, when iTunSMPB stated it.
    pub(super) fn presented_samples(&self) -> Option<i64> {
        self.presented_samples
    }
}

/// Gapless playback as FFmpeg 2da55bf `mp3dec.c` (`mp3_parse_info_tag`)
/// reads it from the first frame of a Layer III stream: after the
/// Xing / Info magic (at the side-info offset, CRC ignored) and the
/// fields its flags select come the 9-byte encoder version, eleven bytes
/// of LAME fields, and the 24-bit encoder delay | padding. Only LAME,
/// Lavf and Lavc encoders are trusted. The frame count, unless the file
/// is much larger than the info frame declares (a concatenation), places
/// the end padding; it is stored in `frames` (else left 0) whether or not
/// the encoder is trusted, for [`itunes`]. `after_header` is the file
/// length from the end of the first frame's header.
pub(super) fn info_tag(frame: &[u8], header: &Mp3FrameHeader, after_header: u64, frames: &mut u32) -> Option<Gapless> {
    if header.layer != Layer::LayerIII {
        return None;
    }
    let be32 = |at: usize| frame.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let lsf = header.version != MpegVersion::Mpeg1;
    let mono = header.channel_count() == 1;
    let mut at = 4 + match (lsf, mono) {
        (false, false) => 32,
        (false, true) | (true, false) => 17,
        (true, true) => 9,
    };
    let magic = frame.get(at..at + 4)?;
    if magic != b"Xing" && magic != b"Info" {
        return None;
    }
    let flags = be32(at + 4)?;
    at += 8;
    if flags & 1 != 0 {
        *frames = be32(at)?;
        at += 4;
    }
    if flags & 2 != 0 {
        let declared = u64::from(be32(at)?);
        at += 4;
        if after_header > 0 && declared > 0 {
            let min = after_header.min(declared);
            if after_header > declared && after_header - min > min >> 4 {
                *frames = 0;
            }
        }
    }
    if flags & 4 != 0 {
        at += 100;
    }
    if flags & 8 != 0 {
        at += 4;
    }
    let version = frame.get(at..at + 4)?;
    if !matches!(version, b"LAME" | b"Lavf" | b"Lavc") {
        return None;
    }
    let delays = frame.get(at + 21..at + 24)?;
    let v = u32::from_be_bytes([0, delays[0], delays[1], delays[2]]);
    let (start_pad, end_pad) = (i64::from(v >> 12), i64::from(v & 0xFFF));
    let samples_per_frame = i64::from(header.samples_per_frame());
    let total = i64::from(*frames) * samples_per_frame;
    Some(Gapless {
        start_skip: (start_pad + 529) as u32,
        window: (*frames > 0).then_some((total - end_pad + 529, total)),
        samples_per_frame,
        presented_samples: None,
    })
}

/// `mp3_parse_itunes_smpb`, for a Layer III stream whose info frame gave
/// no LAME start skip: iTunSMPB's `(priming, remainder, samples)`, used
/// when the priming is at most 16384, there are samples, and, when the
/// info frame counts `frames`, the three add up to what those decode to.
/// Unlike the LAME delay, the priming covers the decoder's delay.
pub(super) fn itunes(smpb: Option<(i64, i64, i64)>, header: &Mp3FrameHeader, frames: u32) -> Option<Gapless> {
    let (priming, remainder, samples) = smpb?;
    let samples_per_frame = i64::from(header.samples_per_frame());
    if header.layer != Layer::LayerIII
        || priming > 16384
        || samples == 0
        || frames != 0 && priming + samples + remainder != i64::from(frames) * samples_per_frame
    {
        return None;
    }
    Some(Gapless {
        start_skip: priming as u32,
        window: Some((priming + samples, priming + samples + remainder)),
        samples_per_frame,
        presented_samples: Some(samples),
    })
}
