//! Keep ordinary WebPKI checks and additionally pin the seedbox leaf certificate.

use std::path::Path;
use std::sync::Arc;

use mediaops_core::SecretSpec;
use mediaops_net::IdentityBundle;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime, pem::PemObject};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};

pub fn load_identity(dir: &Path, spec: &SecretSpec) -> anyhow::Result<IdentityBundle> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let directory = std::fs::metadata(dir)?;
    if !directory.is_dir()
        || directory.uid() != unsafe { libc::geteuid() }
        || directory.permissions().mode() & 0o022 != 0
    {
        anyhow::bail!(
            "TLS directory must be owned by this user and not writable by other users: {}",
            dir.display()
        );
    }
    for name in ["server.key", "client.key"] {
        let path = dir.join(name);
        let meta = std::fs::symlink_metadata(&path)?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.permissions().mode() & 0o077 != 0
        {
            anyhow::bail!(
                "TLS private key {} must be a private regular file owned by this user",
                path.display()
            );
        }
    }
    let identity = IdentityBundle::from_dir(dir)?;
    validate_pins(spec, &identity)?;
    Ok(identity)
}

fn validate_pins(spec: &SecretSpec, identity: &IdentityBundle) -> anyhow::Result<()> {
    for (field, expected, actual) in [
        ("ca_sha256", &spec.ca_sha256, &identity.ca_sha256),
        (
            "server_sha256",
            &spec.server_sha256,
            &identity.server_sha256,
        ),
        (
            "client_sha256",
            &spec.client_sha256,
            &identity.client_sha256,
        ),
    ] {
        // Existing imported Secrets may omit pins; the local TLS bundle still
        // supplies the trust anchor and expected remote leaf in that case.
        if !expected.is_empty() && !expected.eq_ignore_ascii_case(actual) {
            anyhow::bail!("Secret.{field} does not match the configured TLS bundle");
        }
    }
    Ok(())
}

pub fn pinned_client(identity: &IdentityBundle) -> anyhow::Result<Arc<ClientConfig>> {
    let mut config = (*identity.client_config()?).clone();
    config
        .dangerous()
        .set_certificate_verifier(Arc::new(PinnedServer::new(identity)?));
    Ok(Arc::new(config))
}

#[derive(Debug)]
struct PinnedServer {
    webpki: Arc<WebPkiServerVerifier>,
    leaf_sha256: String,
}

impl PinnedServer {
    fn new(identity: &IdentityBundle) -> anyhow::Result<Self> {
        // Initialize the crypto provider through the ordinary bundle path.
        let _ = identity.client_config()?;
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from_pem_slice(identity.ca_pem.as_bytes())?)?;
        Ok(Self {
            webpki: WebPkiServerVerifier::builder(Arc::new(roots)).build()?,
            leaf_sha256: identity.server_sha256.clone(),
        })
    }
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let verified = self.webpki.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        )?;
        let fingerprint: String = Sha256::digest(end_entity.as_ref())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if fingerprint != self.leaf_sha256 {
            return Err(rustls::Error::General(
                "seedbox certificate pin mismatch".into(),
            ));
        }
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.webpki.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.webpki.supported_verify_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mediaops-gateway-tls-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            mediaops_net::mint()
                .expect("identity")
                .write_to_dir(&dir)
                .expect("temporary TLS bundle");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn insecure_key_permissions_or_symlinks_are_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new();
        let spec = SecretSpec::default();
        load_identity(&dir.0, &spec).expect("private identity");
        let key = dir.0.join("client.key");
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(load_identity(&dir.0, &spec).is_err());
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).expect("restore");
        let target = dir.0.join("moved.key");
        std::fs::rename(&key, &target).expect("move");
        std::os::unix::fs::symlink(&target, &key).expect("link");
        assert!(load_identity(&dir.0, &spec).is_err());
    }

    #[test]
    fn every_configured_pin_is_checked_against_the_bundle() {
        let identity = mediaops_net::mint().expect("identity");
        let valid = SecretSpec {
            seedbox_address: "localhost:1234".into(),
            ca_sha256: identity.ca_sha256.to_uppercase(),
            server_sha256: identity.server_sha256.clone(),
            client_sha256: identity.client_sha256.clone(),
        };
        validate_pins(&valid, &identity).expect("matching pins");
        for field in ["ca", "server", "client"] {
            let mut wrong = valid.clone();
            *match field {
                "ca" => &mut wrong.ca_sha256,
                "server" => &mut wrong.server_sha256,
                _ => &mut wrong.client_sha256,
            } = "00".repeat(32);
            assert!(validate_pins(&wrong, &identity).is_err(), "{field}");
        }
    }

    #[test]
    fn remote_leaf_pin_is_enforced_in_addition_to_webpki() {
        let identity = mediaops_net::mint().expect("identity");
        let mut verifier = PinnedServer::new(&identity).expect("verifier");
        let cert =
            CertificateDer::from_pem_slice(identity.server_cert_pem.as_bytes()).expect("cert");
        let name = ServerName::try_from("localhost").expect("server name");
        verifier
            .verify_server_cert(&cert, &[], &name, &[], UnixTime::now())
            .expect("valid chain and pin");
        let wrong_name = ServerName::try_from("attacker.example").expect("name");
        assert!(
            verifier
                .verify_server_cert(&cert, &[], &wrong_name, &[], UnixTime::now())
                .is_err()
        );
        verifier.leaf_sha256 = "00".repeat(32);
        assert!(
            verifier
                .verify_server_cert(&cert, &[], &name, &[], UnixTime::now())
                .is_err()
        );
    }
}
