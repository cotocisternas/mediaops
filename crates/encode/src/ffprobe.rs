//! ffprobe JSON via ExecPort. Unit tests never spawn ffmpeg.

use std::path::Path;

use mediaops_core::{ExecCommand, ExecOutput, ExecPort};
use serde::Deserialize;

use crate::EncodeError;
use crate::policy::{Container, ProbeMedia, VideoCodec};

pub fn ffprobe_command(ffprobe: &str, path: &Path) -> ExecCommand {
    ExecCommand::new(
        ffprobe,
        vec![
            "-v".into(),
            "quiet".into(),
            "-print_format".into(),
            "json".into(),
            "-show_streams".into(),
            "-show_format".into(),
            path.display().to_string(),
        ],
    )
}

pub async fn probe_media(exec: &impl ExecPort, path: &Path) -> Result<ProbeMedia, EncodeError> {
    probe_media_named(exec, "ffprobe", path).await
}

pub async fn probe_media_named(
    exec: &impl ExecPort,
    ffprobe: &str,
    path: &Path,
) -> Result<ProbeMedia, EncodeError> {
    let cmd = ffprobe_command(ffprobe, path);
    let out = exec
        .run(&cmd)
        .await
        .map_err(|err| EncodeError::Exec(err.to_string()))?;
    if out.status != 0 {
        return Err(EncodeError::Exec(format!("ffprobe exited {}", out.status)));
    }
    parse_ffprobe_json(&out)
}

fn parse_ffprobe_json(out: &ExecOutput) -> Result<ProbeMedia, EncodeError> {
    let parsed: FfprobeJson =
        serde_json::from_slice(&out.stdout).map_err(|err| EncodeError::Probe(err.to_string()))?;
    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| EncodeError::Probe("no video stream".into()))?;
    let codec = match video.codec_name.as_deref() {
        Some("hevc") | Some("h265") => VideoCodec::Hevc,
        Some("h264") | Some("avc1") => VideoCodec::H264,
        _ => VideoCodec::Other,
    };
    let bit_depth = video
        .bits_per_raw_sample
        .as_deref()
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            video.pix_fmt.as_deref().and_then(|pix| {
                if pix.contains("10") {
                    Some(10)
                } else if pix.contains("12") {
                    Some(12)
                } else {
                    Some(8)
                }
            })
        })
        .unwrap_or(8);
    let format_name = parsed.format.format_name.unwrap_or_default();
    let container = if format_name
        .split(',')
        .any(|n| matches!(n.trim(), "mp4" | "mov" | "ismv"))
    {
        Container::Mp4
    } else {
        Container::Other
    };
    let hdr = is_hdr(video);
    let dolby_vision = is_dv(video);
    Ok(ProbeMedia {
        codec,
        bit_depth,
        width: video.width.unwrap_or(0),
        height: video.height.unwrap_or(0),
        container,
        hdr,
        dolby_vision,
    })
}

fn is_hdr(video: &FfprobeStream) -> bool {
    matches!(
        video.color_transfer.as_deref(),
        Some("smpte2084") | Some("arib-std-b67")
    ) || video.side_data_list.iter().any(|s| {
        s.side_data_type
            .as_deref()
            .is_some_and(|t| t.to_ascii_lowercase().contains("mastering display"))
    })
}

fn is_dv(video: &FfprobeStream) -> bool {
    video
        .codec_tag_string
        .as_deref()
        .is_some_and(|t| t.to_ascii_lowercase().contains("dvh"))
        || video.side_data_list.iter().any(|s| {
            s.side_data_type
                .as_deref()
                .is_some_and(|t| t.to_ascii_lowercase().contains("dolby vision"))
        })
}

#[derive(Debug, Deserialize)]
struct FfprobeJson {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    #[serde(default)]
    format: FfprobeFormat,
}

#[derive(Debug, Default, Deserialize)]
struct FfprobeFormat {
    format_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    codec_tag_string: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    bits_per_raw_sample: Option<String>,
    pix_fmt: Option<String>,
    color_transfer: Option<String>,
    #[serde(default)]
    side_data_list: Vec<FfprobeSideData>,
}

#[derive(Debug, Deserialize)]
struct FfprobeSideData {
    side_data_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{ExecError, ExecPort};
    use std::path::PathBuf;
    use std::sync::Mutex;

    struct Transcript {
        calls: Mutex<Vec<ExecCommand>>,
        stdout: String,
        status: i32,
    }

    impl ExecPort for Transcript {
        async fn run(&self, command: &ExecCommand) -> Result<ExecOutput, ExecError> {
            self.calls.lock().expect("calls").push(command.clone());
            Ok(ExecOutput {
                status: self.status,
                stdout: self.stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    const HEVC10_MP4: &str = r#"{
        "streams": [{
            "codec_type": "video",
            "codec_name": "hevc",
            "width": 1920,
            "height": 1080,
            "bits_per_raw_sample": "10",
            "pix_fmt": "yuv420p10le",
            "color_transfer": "bt709"
        }],
        "format": { "format_name": "mov,mp4,m4a,3gp,3g2,mj2" }
    }"#;

    #[tokio::test]
    async fn ffprobe_argv_is_json_show_streams_and_format() {
        let exec = Transcript {
            calls: Mutex::new(Vec::new()),
            stdout: HEVC10_MP4.into(),
            status: 0,
        };
        let path = PathBuf::from("/lib/movies/x.mp4");
        let media = probe_media(&exec, &path).await.expect("probe");
        assert_eq!(media.codec, VideoCodec::Hevc);
        assert_eq!(media.bit_depth, 10);
        assert_eq!(media.container, Container::Mp4);
        assert!(!media.hdr);
        let calls = exec.calls.lock().expect("calls").clone();
        assert_eq!(calls[0].program, "ffprobe");
        assert_eq!(
            calls[0].args,
            vec![
                "-v",
                "quiet",
                "-print_format",
                "json",
                "-show_streams",
                "-show_format",
                "/lib/movies/x.mp4"
            ]
        );
    }
}
