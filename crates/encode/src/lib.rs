//! EncodePolicy, ffprobe, and NVENC transcode. Linked only into the CLI tree.

use mediaops_core::{ExecCommand, ExecPort};

pub mod ffprobe;
pub mod policy;
pub mod run;

pub use ffprobe::{ffprobe_command, probe_media};
pub use policy::{Container, EncodeDecision, ProbeMedia, VideoCodec, classify};
pub use run::{
    TranscodeSpec, backup_path, converting_path, encode_to_converting, ffmpeg_nvenc_args,
    replace_converting, session_cap, should_start_next, transcode_nvenc,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvencProbe {
    pub cap: u32,
    pub ffmpeg_path: String,
    pub hevc: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    #[error("{0}")]
    Exec(String),
    #[error("ffprobe: {0}")]
    Probe(String),
    #[error("path: {0}")]
    Path(String),
    #[error("install: {0}")]
    Install(String),
    #[error("io: {0}")]
    Io(String),
    #[error("encode paused")]
    Paused,
    #[error("no NVENC capacity")]
    NoCapacity,
    #[error("encode refused by policy")]
    Refused,
}

/// Probe `ffmpeg -encoders` for `hevc_nvenc`. No GPU required for the fake exec port.
pub async fn probe_nvenc(exec: &impl ExecPort) -> Result<NvencProbe, EncodeError> {
    let cmd = ExecCommand::new("ffmpeg", vec!["-hide_banner".into(), "-encoders".into()]);
    match exec.run(&cmd).await {
        Ok(out) if out.status == 0 => {
            let text = String::from_utf8_lossy(&out.stdout).to_string()
                + &String::from_utf8_lossy(&out.stderr);
            let hevc = text.contains("hevc_nvenc");
            Ok(NvencProbe {
                cap: u32::from(hevc),
                ffmpeg_path: cmd.program.clone(),
                hevc,
            })
        }
        Ok(_) | Err(_) => Ok(NvencProbe {
            cap: 0,
            ffmpeg_path: String::new(),
            hevc: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaops_core::{ExecError, ExecOutput};

    struct Fake {
        stdout: &'static str,
        fail: bool,
        status: i32,
    }

    impl ExecPort for Fake {
        async fn run(&self, command: &ExecCommand) -> Result<ExecOutput, ExecError> {
            assert_eq!(command.program, "ffmpeg");
            if self.fail {
                return Err(ExecError::Failed {
                    program: "ffmpeg".into(),
                    message: "not found".into(),
                });
            }
            Ok(ExecOutput {
                status: self.status,
                stdout: self.stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    #[tokio::test]
    async fn hevc_nvenc_counts_as_cap_one() {
        let probe = probe_nvenc(&Fake {
            stdout: " V..... hevc_nvenc           NVIDIA NVENC hevc encoder",
            fail: false,
            status: 0,
        })
        .await
        .expect("probe");
        assert_eq!(probe.cap, 1);
        assert!(probe.hevc);
    }

    #[tokio::test]
    async fn missing_ffmpeg_is_cap_zero() {
        let probe = probe_nvenc(&Fake {
            stdout: "",
            fail: true,
            status: 0,
        })
        .await
        .expect("probe");
        assert_eq!(probe.cap, 0);
        assert!(!probe.hevc);
    }

    #[tokio::test]
    async fn nonzero_ffmpeg_status_is_cap_zero() {
        let probe = probe_nvenc(&Fake {
            stdout: " V..... hevc_nvenc           NVIDIA NVENC hevc encoder",
            fail: false,
            status: 1,
        })
        .await
        .expect("probe");
        assert_eq!(probe.cap, 0);
        assert!(!probe.hevc);
    }
}
