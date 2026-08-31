//! NVENC probe. Encode policy waits for Epic 4.

use mediaops_core::{ExecCommand, ExecPort};

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
