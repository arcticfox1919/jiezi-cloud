//! TLS configuration for the QUIC server.
//!
//! In production, the operator supplies a certificate chain and private key
//! as PEM strings in `config.toml` (`[quic] tls_cert_pem` / `tls_key_pem`).
//!
//! In development mode (or when either field is set to the sentinel value
//! `"GENERATE"`), a self-signed certificate is generated on the fly using
//! [`rcgen`].  The certificate is valid for one year and uses P-256 ECDSA.
//!
//! # Why P-256?
//!
//! `rcgen` supports P-256 (ECDSA) by default and it pairs well with the
//! ES256 JWT keys already used by the auth module.

use std::sync::Arc;

use anyhow::{Context, Result};
use quinn::ServerConfig;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType, date_time_ymd};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tracing::info;

use jiezi_cloud_config::QuicConfig;

// ─── Public entry point ───────────────────────────────────────────────────────

/// Build a [`quinn::ServerConfig`] from `cfg`.
///
/// - If `cfg.tls_cert_pem` or `cfg.tls_key_pem` is `"GENERATE"`, generates an
///   ephemeral self-signed certificate at runtime.
/// - Otherwise, parses the PEM strings from the configuration.
///
/// # Errors
///
/// Returns an error if the PEM strings are malformed or if crypto
/// initialisation fails.
pub fn build_server_tls(cfg: &QuicConfig) -> Result<ServerConfig> {
    let (cert_der, key_der) = if cfg.tls_cert_pem == "GENERATE" || cfg.tls_key_pem == "GENERATE" {
        generate_self_signed()?
    } else {
        load_from_pem(&cfg.tls_cert_pem, &cfg.tls_key_pem)?
    };

    make_server_config(cert_der, key_der, cfg.max_concurrent_bidi_streams)
}

// ─── Certificate generation ──────────────────────────────────────────────────

/// Generate an ephemeral ECDSA P-256 self-signed certificate.
///
/// Returns `(cert_der, pkcs8_key_der)`.
fn generate_self_signed() -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    info!("QUIC: generating ephemeral self-signed TLS certificate (dev mode)");

    let key_pair = KeyPair::generate()?;

    let mut params = CertificateParams::default();

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "jiezi-cloud-dev");
    dn.push(DnType::OrganizationName, "Jiezi Cloud (dev)");
    params.distinguished_name = dn;

    params.subject_alt_names = vec![
        SanType::DnsName("localhost".try_into().context("invalid SAN: localhost")?)  ,
        SanType::IpAddress(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)),
        SanType::IpAddress(std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)),
    ];

    // 10-year validity so devs don't wake up to a broken dev environment.
    params.not_before = date_time_ymd(2024, 1, 1);
    params.not_after = date_time_ymd(2034, 1, 1);

    let cert = params.self_signed(&key_pair)?;

    let cert_der = CertificateDer::from(cert.der().to_vec());
    let key_der  = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_pair.serialize_der()));

    info!("QUIC: ephemeral TLS certificate generated");

    Ok((cert_der, key_der))
}

/// Parse PEM certificate + private key from strings.
fn load_from_pem(
    cert_pem: &str,
    key_pem: &str,
) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    use rustls_pemfile::{certs, pkcs8_private_keys};
    use std::io::BufReader;

    let cert_der = certs(&mut BufReader::new(cert_pem.as_bytes()))
        .next()
        .context("no certificate in tls_cert_pem")?
        .context("failed to parse certificate")
        .map(|c| c.into_owned())?;

    let key_der = pkcs8_private_keys(&mut BufReader::new(key_pem.as_bytes()))
        .next()
        .context("no PKCS#8 key in tls_key_pem")?
        .context("failed to parse private key")
        .map(|k| PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(k.secret_pkcs8_der().to_vec())))?;

    Ok((cert_der, key_der))
}

// ─── ServerConfig assembly ────────────────────────────────────────────────────

fn make_server_config(
    cert_der: CertificateDer<'static>,
    key_der: PrivateKeyDer<'static>,
    max_bidi_streams: u32,
) -> Result<ServerConfig> {
    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .context("failed to build rustls ServerConfig")?;

    // QUIC requires TLS 1.3.
    tls_config.alpn_protocols = vec![b"jtp/1".to_vec()];
    tls_config.max_early_data_size = u32::MAX; // allow 0-RTT

    let transport_config = {
        let mut tc = quinn::TransportConfig::default();
        tc.max_concurrent_bidi_streams(max_bidi_streams.into());
        tc.max_concurrent_uni_streams(0u32.into()); // server opens uni streams itself
        Arc::new(tc)
    };

    let mut server_config = ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls_config)
            .context("failed to build quinn crypto config")?,
    ));
    server_config.transport_config(transport_config);

    Ok(server_config)
}
