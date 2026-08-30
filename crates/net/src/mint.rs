//! TLS identity minting (AD-14). ECDSA P-256 CA + server + client.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use mediaops_core::TlsIdentity;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, PKCS_ECDSA_P256_SHA256,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, pem::PemObject};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use sha2::{Digest, Sha256};

use crate::NetError;

pub const SERVER_NAME: &str = "localhost";

pub struct IdentityBundle {
    pub ca_pem: String,
    pub server_cert_pem: String,
    pub server_key_pem: String,
    pub client_cert_pem: String,
    pub client_key_pem: String,
    pub ca_sha256: String,
    pub server_sha256: String,
    pub client_sha256: String,
}

fn sha256_hex(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn ecdsa_key() -> Result<KeyPair, NetError> {
    KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).map_err(|err| NetError::Mint(err.to_string()))
}

pub fn ensure_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

pub fn mint() -> Result<IdentityBundle, NetError> {
    ensure_crypto_provider();
    let ca_key = ecdsa_key()?;
    let mut ca_params =
        CertificateParams::new(Vec::new()).map_err(|err| NetError::Mint(err.to_string()))?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "mediaops-ca");
    ca_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .map_err(|err| NetError::Mint(err.to_string()))?;
    let ca_pem = ca_cert.pem();
    let ca_sha256 = sha256_hex(ca_cert.der().as_ref());
    let issuer = Issuer::new(ca_params, ca_key);

    let server_key = ecdsa_key()?;
    let mut server_params = CertificateParams::new(vec!["localhost".into(), "mediaops".into()])
        .map_err(|err| NetError::Mint(err.to_string()))?;
    server_params
        .distinguished_name
        .push(DnType::CommonName, "mediaops-server");
    server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_cert = server_params
        .signed_by(&server_key, &issuer)
        .map_err(|err| NetError::Mint(err.to_string()))?;

    let client_key = ecdsa_key()?;
    let mut client_params = CertificateParams::new(vec!["localhost".into(), "mediaops".into()])
        .map_err(|err| NetError::Mint(err.to_string()))?;
    client_params
        .distinguished_name
        .push(DnType::CommonName, "mediaops-client");
    client_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    client_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let client_cert = client_params
        .signed_by(&client_key, &issuer)
        .map_err(|err| NetError::Mint(err.to_string()))?;

    Ok(IdentityBundle {
        ca_pem,
        server_cert_pem: server_cert.pem(),
        server_key_pem: server_key.serialize_pem(),
        client_cert_pem: client_cert.pem(),
        client_key_pem: client_key.serialize_pem(),
        ca_sha256,
        server_sha256: sha256_hex(server_cert.der().as_ref()),
        client_sha256: sha256_hex(client_cert.der().as_ref()),
    })
}

impl IdentityBundle {
    pub fn write_to_dir(&self, dir: &Path) -> Result<TlsIdentity, NetError> {
        fs::create_dir_all(dir).map_err(|err| NetError::Io(err.to_string()))?;
        let ca_path = dir.join("ca.pem");
        let server_cert_path = dir.join("server.pem");
        let server_key_path = dir.join("server.key");
        let client_cert_path = dir.join("client.pem");
        let client_key_path = dir.join("client.key");
        fs::write(&ca_path, &self.ca_pem).map_err(|err| NetError::Io(err.to_string()))?;
        fs::write(&server_cert_path, &self.server_cert_pem)
            .map_err(|err| NetError::Io(err.to_string()))?;
        fs::write(&server_key_path, &self.server_key_pem)
            .map_err(|err| NetError::Io(err.to_string()))?;
        fs::write(&client_cert_path, &self.client_cert_pem)
            .map_err(|err| NetError::Io(err.to_string()))?;
        fs::write(&client_key_path, &self.client_key_pem)
            .map_err(|err| NetError::Io(err.to_string()))?;
        Ok(TlsIdentity {
            ca_path: path_string(&ca_path),
            server_cert_path: path_string(&server_cert_path),
            server_key_path: path_string(&server_key_path),
            client_cert_path: path_string(&client_cert_path),
            client_key_path: path_string(&client_key_path),
            ca_sha256: self.ca_sha256.clone(),
            server_sha256: self.server_sha256.clone(),
            client_sha256: self.client_sha256.clone(),
        })
    }

    pub fn from_dir(dir: &Path) -> Result<Self, NetError> {
        let ca_pem = fs::read_to_string(dir.join("ca.pem")).map_err(|err| NetError::Io(err.to_string()))?;
        let server_cert_pem =
            fs::read_to_string(dir.join("server.pem")).map_err(|err| NetError::Io(err.to_string()))?;
        let server_key_pem =
            fs::read_to_string(dir.join("server.key")).map_err(|err| NetError::Io(err.to_string()))?;
        let client_cert_pem =
            fs::read_to_string(dir.join("client.pem")).map_err(|err| NetError::Io(err.to_string()))?;
        let client_key_pem =
            fs::read_to_string(dir.join("client.key")).map_err(|err| NetError::Io(err.to_string()))?;
        Ok(Self {
            ca_sha256: sha256_hex(cert_der(&ca_pem)?.as_ref()),
            server_sha256: sha256_hex(cert_der(&server_cert_pem)?.as_ref()),
            client_sha256: sha256_hex(cert_der(&client_cert_pem)?.as_ref()),
            ca_pem,
            server_cert_pem,
            server_key_pem,
            client_cert_pem,
            client_key_pem,
        })
    }

    pub fn server_config(&self) -> Result<Arc<ServerConfig>, NetError> {
        ensure_crypto_provider();
        let mut roots = RootCertStore::empty();
        roots
            .add(cert_der(&self.ca_pem)?)
            .map_err(|err| NetError::Tls(err.to_string()))?;
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|err| NetError::Tls(err.to_string()))?;
        let certs = vec![cert_der(&self.server_cert_pem)?];
        let key = key_der(&self.server_key_pem)?;
        let mut config = ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)
            .map_err(|err| NetError::Tls(err.to_string()))?;
        config.alpn_protocols = vec![b"h2".to_vec()];
        Ok(Arc::new(config))
    }

    pub fn client_config(&self) -> Result<Arc<ClientConfig>, NetError> {
        ensure_crypto_provider();
        let mut roots = RootCertStore::empty();
        roots
            .add(cert_der(&self.ca_pem)?)
            .map_err(|err| NetError::Tls(err.to_string()))?;
        let certs = vec![cert_der(&self.client_cert_pem)?];
        let key = key_der(&self.client_key_pem)?;
        let mut config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(certs, key)
            .map_err(|err| NetError::Tls(err.to_string()))?;
        config.alpn_protocols = vec![b"h2".to_vec()];
        Ok(Arc::new(config))
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn cert_der(pem: &str) -> Result<CertificateDer<'static>, NetError> {
    CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|err| NetError::Tls(err.to_string()))
}

fn key_der(pem: &str) -> Result<PrivateKeyDer<'static>, NetError> {
    PrivateKeyDer::from_pem_slice(pem.as_bytes()).map_err(|err| NetError::Tls(err.to_string()))
}
