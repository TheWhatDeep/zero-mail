//! HTTPS for autoconfig lookups (SPEC_GAPS G29), through `reqwest` on rustls with the `ring`
//! provider and the OS verifier. No cookies (the feature is off), no referrer, no proxy
//! discovery, a generic user agent, HTTPS only (redirects included), a 10 s timeout and a
//! size cap.

use std::sync::Arc;
use std::time::Duration;

use crate::autoconfig::{HttpGet, AUTOCONFIG_MAX_BYTES};
use crate::AuthError;

const TIMEOUT: Duration = Duration::from_secs(10);
const USER_AGENT: &str = "Mozilla/5.0";

/// The HTTP client for autoconfig lookups.
#[derive(Debug, Clone)]
pub struct HttpClient {
    client: reqwest::Client,
}

impl HttpClient {
    pub fn new() -> Result<Self, AuthError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = rustls_platform_verifier::Verifier::new(Arc::clone(&provider))
            .map_err(|_| AuthError::Unsupported("TLS verification"))?;
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| AuthError::Unsupported("TLS 1.2"))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        let client = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .https_only(true)
            .referer(false)
            .no_proxy()
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()
            .map_err(|_| AuthError::Unsupported("HTTP client"))?;
        Ok(Self { client })
    }
}

impl HttpGet for HttpClient {
    async fn get(&self, url: &str) -> Option<Vec<u8>> {
        let mut resp = self.client.get(url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        if resp
            .content_length()
            .is_some_and(|n| n > AUTOCONFIG_MAX_BYTES as u64)
        {
            return None;
        }
        let mut body = Vec::new();
        while let Some(chunk) = resp.chunk().await.ok()? {
            body.extend_from_slice(&chunk);
            if body.len() > AUTOCONFIG_MAX_BYTES {
                return None;
            }
        }
        Some(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds() {
        assert!(HttpClient::new().is_ok());
    }

    #[tokio::test]
    async fn plain_http_is_refused() {
        // https_only: an http:// URL fails before any connection is made.
        let client = HttpClient::new().unwrap();
        assert_eq!(client.get("http://127.0.0.1:1/x").await, None);
    }
}
