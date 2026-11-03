//! Local certificate authority and host-based dispatch for servers that only speak HTTPS to
//! a fixed vendor hostname. Compose gives the world container network aliases for those
//! hostnames; the tools container trusts the CA fetched from `/_bench/ca.pem`.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, StatusCode, header::HOST},
    response::{IntoResponse, Response},
};
use mock_core::{http::json, serve::service_app, service::MockService, world::SharedWorld};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use serde_json::json as json_value;
use tower::ServiceExt;

/// A freshly generated CA plus one leaf certificate whose SANs are exactly `hosts`.
/// Keys live only in memory for the life of the world process.
pub struct LocalCa {
    pub ca_pem: String,
    pub leaf_cert_pem: String,
    pub leaf_key_pem: String,
}

impl LocalCa {
    pub fn generate(hosts: &[&str]) -> anyhow::Result<Self> {
        let ca_key = KeyPair::generate()?;
        let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "tool-bench local CA");
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let ca_cert = ca_params.self_signed(&ca_key)?;
        let issuer = Issuer::new(ca_params, ca_key);

        let leaf_key = KeyPair::generate()?;
        let mut leaf_params =
            CertificateParams::new(hosts.iter().map(|h| (*h).to_owned()).collect::<Vec<_>>())?;
        leaf_params
            .distinguished_name
            .push(DnType::CommonName, "tool-bench world");
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        // Python 3.13+ verifies with VERIFY_X509_STRICT, which requires an AKI.
        leaf_params.use_authority_key_identifier_extension = true;
        let leaf_cert = leaf_params.signed_by(&leaf_key, &issuer)?;

        Ok(Self {
            ca_pem: ca_cert.pem(),
            leaf_cert_pem: leaf_cert.pem(),
            leaf_key_pem: leaf_key.serialize_pem(),
        })
    }
}

/// Every real hostname declared by `services`, sorted and de-duplicated.
pub fn all_hosts(services: &[Box<dyn MockService>]) -> Vec<&'static str> {
    let mut hosts: Vec<&'static str> = services.iter().flat_map(|s| s.hosts()).copied().collect();
    hosts.sort_unstable();
    hosts.dedup();
    hosts
}

/// Routes each request to the service that owns its `Host`, marking it as HTTPS.
/// Unknown hosts get 421 Misdirected Request.
pub fn host_router(world: SharedWorld, services: &[Box<dyn MockService>]) -> Router {
    let mut routes = BTreeMap::new();
    for service in services {
        let app = service_app(world.clone(), service.as_ref());
        for host in service.hosts() {
            routes.insert(host.to_ascii_lowercase(), app.clone());
        }
    }
    let routes = Arc::new(routes);
    Router::new().fallback(move |request: Request| {
        let routes = routes.clone();
        async move { dispatch(&routes, request).await }
    })
}

async fn dispatch(routes: &BTreeMap<String, Router>, mut request: Request) -> Response {
    let host = request_host(&request);
    let Some(app) = routes.get(&host).cloned() else {
        return json(
            StatusCode::MISDIRECTED_REQUEST,
            json_value!({ "message": "no mock service for this host", "host": host }),
        );
    };
    request
        .headers_mut()
        .insert("x-forwarded-proto", HeaderValue::from_static("https"));
    match app.oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    }
    .into_response()
}

/// Lowercased host without port, from `Host` (HTTP/1.1) or the URI authority (HTTP/2).
fn request_host(request: &Request) -> String {
    let raw = request
        .headers()
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| request.uri().authority().map(|a| a.as_str()))
        .unwrap_or_default();
    raw.rsplit_once(':')
        .map_or(raw, |(host, _port)| host)
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DER encoding of OID 2.5.29.35 (authorityKeyIdentifier).
    const AKI_OID: [u8; 5] = [0x06, 0x03, 0x55, 0x1d, 0x23];

    fn der(pem: &str) -> Vec<u8> {
        let body: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
        let table = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let vals: Vec<u32> = body
            .bytes()
            .filter(|b| *b != b'=')
            .map(|b| table.iter().position(|t| *t == b).unwrap() as u32)
            .collect();
        let mut out = Vec::new();
        for chunk in vals.chunks(4) {
            let n = chunk
                .iter()
                .enumerate()
                .fold(0, |acc, (i, v)| acc | (v << (18 - 6 * i)));
            for i in 0..chunk.len() - 1 {
                out.push((n >> (16 - 8 * i)) as u8);
            }
        }
        out
    }

    #[test]
    fn leaf_carries_an_authority_key_identifier() {
        let ca = LocalCa::generate(&["octo-corp.okta.com"]).unwrap();
        let leaf = der(&ca.leaf_cert_pem);
        assert!(
            leaf.windows(AKI_OID.len()).any(|w| w == AKI_OID),
            "leaf has no AKI extension"
        );
    }
}
