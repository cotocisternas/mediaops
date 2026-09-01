//! v1 EncodePolicy. Hardcoded matrix; not a desired-state field.

use mediaops_core::TitleKind;

/// Classification input. Unit tests pass this struct; they never run ffmpeg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeMedia {
    pub codec: VideoCodec,
    pub bit_depth: u8,
    pub width: u32,
    pub height: u32,
    pub container: Container,
    pub hdr: bool,
    pub dolby_vision: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Hevc,
    H264,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Mp4,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeDecision {
    Keep,
    NvencH264,
    Refuse,
}

impl EncodeDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::NvencH264 => "nvenc_h264",
            Self::Refuse => "refuse",
        }
    }
}

/// Named v1 rules. HDR/DV and 2160p remux are Refuse (keep-forever).
pub fn classify(kind: TitleKind, media: &ProbeMedia) -> EncodeDecision {
    if media.hdr || media.dolby_vision {
        return EncodeDecision::Refuse;
    }
    if media.height >= 2160 {
        return EncodeDecision::Refuse;
    }
    if kind == TitleKind::Series
        && media.codec == VideoCodec::Hevc
        && media.container == Container::Mp4
    {
        return EncodeDecision::Keep;
    }
    if kind == TitleKind::Movie
        && media.codec == VideoCodec::Hevc
        && media.bit_depth >= 10
        && media.container == Container::Mp4
        && !media.hdr
    {
        return EncodeDecision::NvencH264;
    }
    if media.codec == VideoCodec::H264 && media.bit_depth <= 8 {
        return EncodeDecision::Keep;
    }
    EncodeDecision::Keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn movie_hevc10_mp4() -> ProbeMedia {
        ProbeMedia {
            codec: VideoCodec::Hevc,
            bit_depth: 10,
            width: 1920,
            height: 1080,
            container: Container::Mp4,
            hdr: false,
            dolby_vision: false,
        }
    }

    #[test]
    fn movie_hevc10_mp4_is_nvenc_h264_hevc_mp4_chrome_dropped_frames() {
        assert_eq!(
            classify(TitleKind::Movie, &movie_hevc10_mp4()),
            EncodeDecision::NvencH264
        );
    }

    #[test]
    fn series_hevc_mp4_is_keep_named_series_skip() {
        let media = ProbeMedia {
            codec: VideoCodec::Hevc,
            bit_depth: 10,
            width: 1920,
            height: 1080,
            container: Container::Mp4,
            hdr: false,
            dolby_vision: false,
        };
        assert_eq!(classify(TitleKind::Series, &media), EncodeDecision::Keep);
    }

    #[test]
    fn hdr_or_dv_is_refuse_keep_forever() {
        let mut hdr = movie_hevc10_mp4();
        hdr.hdr = true;
        assert_eq!(classify(TitleKind::Movie, &hdr), EncodeDecision::Refuse);
        let mut dv = movie_hevc10_mp4();
        dv.dolby_vision = true;
        assert_eq!(classify(TitleKind::Movie, &dv), EncodeDecision::Refuse);
    }

    #[test]
    fn height_2160_is_refuse() {
        let mut uhd = movie_hevc10_mp4();
        uhd.height = 2160;
        assert_eq!(classify(TitleKind::Movie, &uhd), EncodeDecision::Refuse);
    }

    #[test]
    fn h264_8bit_already_is_keep() {
        let media = ProbeMedia {
            codec: VideoCodec::H264,
            bit_depth: 8,
            width: 1920,
            height: 1080,
            container: Container::Mp4,
            hdr: false,
            dolby_vision: false,
        };
        assert_eq!(classify(TitleKind::Movie, &media), EncodeDecision::Keep);
    }
}
