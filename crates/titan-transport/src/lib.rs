//! TLS identities, pinning, bounded control records and peer authentication.
use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::RngCore;
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use titan_protocol::{CONTROL_LIMIT, Control};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0; N];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}
pub fn fingerprint(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}
pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        })
        .join("titancam")
}
fn private_write(path: &Path, b: &[u8]) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    use std::io::Write;
    f.write_all(b)?;
    f.sync_all()?;
    Ok(())
}
pub struct Identity {
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
    pub signing: SigningKey,
    pub directory: PathBuf,
}
impl Identity {
    pub fn load(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        let cert_path = dir.join("tls.der");
        let key_path = dir.join("tls-key.der");
        if !cert_path.exists() {
            let rcgen::CertifiedKey { cert, key_pair } =
                rcgen::generate_simple_self_signed(vec!["titancam.local".into()])?;
            private_write(&key_path, &key_pair.serialize_der())?;
            private_write(&cert_path, cert.der().as_ref())?;
        }
        let seed_path = dir.join("identity.seed");
        if !seed_path.exists() {
            private_write(&seed_path, &random::<32>())?;
        }
        let seed: [u8; 32] = fs::read(seed_path)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid stored identity seed"))?;
        Ok(Self {
            cert: fs::read(cert_path)?,
            key: fs::read(key_path)?,
            signing: SigningKey::from_bytes(&seed),
            directory: dir,
        })
    }
    pub fn public(&self) -> [u8; 32] {
        self.signing.verifying_key().to_bytes()
    }
    pub fn tls_server(&self, alpn: &[u8]) -> Result<rustls::ServerConfig> {
        let mut c = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(self.cert.clone())],
                PrivatePkcs8KeyDer::from(self.key.clone()).into(),
            )?;
        c.alpn_protocols = vec![alpn.to_vec()];
        Ok(c)
    }
    pub fn known(&self, pubkey: &[u8; 32]) -> bool {
        self.directory
            .join(format!("peer-{}.pub", hex::encode(pubkey)))
            .exists()
    }
    pub fn remember(&self, pubkey: &[u8; 32]) -> Result<()> {
        let path = self
            .directory
            .join(format!("peer-{}.pub", hex::encode(pubkey)));
        if !path.exists() {
            private_write(&path, pubkey)?;
        }
        Ok(())
    }
    pub fn revoke(&self, pubkey: &str) -> Result<()> {
        let _: [u8; 32] = hex_array(pubkey)?;
        fs::remove_file(self.directory.join(format!("peer-{pubkey}.pub")))?;
        Ok(())
    }
}
pub fn hex_array<const N: usize>(s: &str) -> Result<[u8; N]> {
    hex::decode(s)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected {N}-byte hex value"))
}
pub fn transcript(
    role: u8,
    phone: &[u8; 32],
    receiver: &[u8; 32],
    nonce: &[u8; 32],
    session: &[u8; 16],
) -> Vec<u8> {
    let mut b = b"TitanCam-auth-v1".to_vec();
    b.push(role);
    b.extend(Sha256::digest(phone));
    b.extend(Sha256::digest(receiver));
    b.extend(nonce);
    b.extend(session);
    b
}
pub fn sign(
    key: &SigningKey,
    role: u8,
    phone: &[u8; 32],
    receiver: &[u8; 32],
    nonce: &[u8; 32],
    session: &[u8; 16],
) -> String {
    hex::encode(
        key.sign(&transcript(role, phone, receiver, nonce, session))
            .to_bytes(),
    )
}
pub fn verify(
    public: &[u8; 32],
    signature: &str,
    role: u8,
    phone: &[u8; 32],
    receiver: &[u8; 32],
    nonce: &[u8; 32],
    session: &[u8; 16],
) -> Result<()> {
    let sig: [u8; 64] = hex_array(signature)?;
    VerifyingKey::from_bytes(public)?
        .verify(
            &transcript(role, phone, receiver, nonce, session),
            &Signature::from_bytes(&sig),
        )
        .context("peer authentication failed")
}
pub fn secret_matches(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |v, (a, b)| v | (a ^ b)) == 0
}
#[derive(Debug)]
struct PinVerifier {
    pin: String,
}
impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        _chain: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        if fingerprint(end.as_ref()) != self.pin {
            return Err(rustls::Error::General("certificate pin mismatch".into()));
        }
        let (_, cert) = x509_parser::parse_x509_certificate(end.as_ref())
            .map_err(|_| rustls::Error::General("invalid X509 certificate".into()))?;
        if !cert.validity().is_valid() {
            return Err(rustls::Error::General(
                "certificate expired or not yet valid".into(),
            ));
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
pub fn tls_client(pin: &str, alpn: &[u8]) -> Result<rustls::ClientConfig> {
    let _: [u8; 32] = hex_array(pin)?;
    let mut c = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinVerifier { pin: pin.into() }))
        .with_no_client_auth();
    c.alpn_protocols = vec![alpn.to_vec()];
    c.enable_early_data = false;
    Ok(c)
}
fn json_depth(v: &serde_json::Value, n: usize) -> bool {
    if n > 16 {
        return false;
    }
    match v {
        serde_json::Value::Array(a) => a.len() <= 256 && a.iter().all(|v| json_depth(v, n + 1)),
        serde_json::Value::Object(o) => o.len() <= 256 && o.values().all(|v| json_depth(v, n + 1)),
        _ => true,
    }
}
pub async fn read_control<R: AsyncRead + Unpin>(r: &mut R) -> Result<Control> {
    let n = r.read_u32().await? as usize;
    ensure!(n > 0 && n <= CONTROL_LIMIT, "control record exceeds limit");
    let mut b = vec![0; n];
    r.read_exact(&mut b).await?;
    let c: Control = serde_json::from_slice(&b)?;
    ensure!(
        c.version == 1 && c.kind.len() <= 64 && c.session_id.len() <= 32 && json_depth(&c.body, 0),
        "invalid control envelope"
    );
    Ok(c)
}
pub async fn write_control<W: AsyncWrite + Unpin>(w: &mut W, c: &Control) -> Result<()> {
    let b = serde_json::to_vec(c)?;
    ensure!(b.len() <= CONTROL_LIMIT, "control record exceeds limit");
    w.write_u32(b.len() as u32).await?;
    w.write_all(&b).await?;
    w.flush().await?;
    Ok(())
}
pub async fn read_media<R: AsyncRead + Unpin>(r: &mut R) -> Result<Vec<u8>> {
    let n = r.read_u32().await? as usize;
    if !(65..=titan_protocol::VIDEO_LIMIT + 64).contains(&n) {
        bail!("invalid media record length")
    }
    let mut h = [0; 64];
    r.read_exact(&mut h).await?;
    let header = titan_protocol::MediaHeader::parse_with_payload(&h, n - 64)?;
    ensure!(
        header.count == 1
            && header.index == 0
            && header.offset == 0
            && header.unit_len as usize == n - 64,
        "invalid USB media framing"
    );
    let mut b = Vec::with_capacity(n);
    b.extend(h);
    b.resize(n, 0);
    r.read_exact(&mut b[64..]).await?;
    Ok(b)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signatures_reject_replay_and_reflection() {
        let p = SigningKey::from_bytes(&random());
        let r = SigningKey::from_bytes(&random());
        let pp = p.verifying_key().to_bytes();
        let rp = r.verifying_key().to_bytes();
        let nonce = random();
        let sid = random();
        let s = sign(&p, 1, &pp, &rp, &nonce, &sid);
        assert!(verify(&pp, &s, 1, &pp, &rp, &nonce, &sid).is_ok());
        assert!(verify(&pp, &s, 2, &pp, &rp, &nonce, &sid).is_err());
        assert!(verify(&pp, &s, 1, &pp, &rp, &random(), &sid).is_err());
    }
    #[tokio::test]
    async fn partial_records_and_hostile_length() {
        let (mut a, mut b) = tokio::io::duplex(128);
        let task = tokio::spawn(async move {
            a.write_all(&[0, 0]).await.unwrap();
            a.write_all(&[0, 1, b'{']).await.unwrap()
        });
        assert!(read_control(&mut b).await.is_err());
        task.await.unwrap();
        let (mut a, mut b) = tokio::io::duplex(16);
        a.write_u32(u32::MAX).await.unwrap();
        assert!(read_control(&mut b).await.is_err());
    }
    #[test]
    fn wrong_pin_rejected() {
        let rcgen::CertifiedKey { cert, .. } =
            rcgen::generate_simple_self_signed(vec!["titancam.local".into()]).unwrap();
        let v = PinVerifier {
            pin: "00".repeat(32),
        };
        assert!(
            v.verify_server_cert(
                cert.der(),
                &[],
                &ServerName::try_from("titancam.local").unwrap(),
                &[],
                UnixTime::now()
            )
            .is_err()
        )
    }
}
