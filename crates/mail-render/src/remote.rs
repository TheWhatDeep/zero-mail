//! The `mail-remote` proxy (section 8, Remote images): the only way anything in a message
//! reaches the network, and only after the user allowed images for it (the CSP lists
//! `mail-remote:` only for `images=1`). No cookies (reqwest's cookie store is not built), no
//! referrer, a generic user agent, a 10 s timeout, a 5 MB cap, `image/*` responses only, and
//! at most 4 requests at a time in total, which keeps every message within its 4 (SPEC_GAPS
//! G57).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::urls;

const TIMEOUT: Duration = Duration::from_secs(10);
/// The largest image fetched.
pub const MAX_BYTES: usize = 5 * 1024 * 1024;
/// Requests in flight at once.
pub const CONCURRENT: usize = 4;
const USER_AGENT: &str = "Mozilla/5.0";

/// A fetched image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    #[error("not an http(s) URL")]
    BadUrl,
    #[error("the request failed")]
    Failed,
    #[error("the response is not an image")]
    NotImage,
    #[error("the image is larger than 5 MB")]
    TooLarge,
    #[error("the HTTP client could not be built")]
    Client,
}

/// The remote-image proxy. Cheap to clone.
#[derive(Debug, Clone)]
pub struct RemoteProxy {
    client: reqwest::Client,
    slots: Arc<Semaphore>,
}

impl RemoteProxy {
    pub fn new() -> Result<Self, RemoteError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = rustls_platform_verifier::Verifier::new(Arc::clone(&provider))
            .map_err(|_| RemoteError::Client)?;
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|_| RemoteError::Client)?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(verifier))
            .with_no_client_auth();
        let client = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .referer(false)
            .no_proxy()
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|_| RemoteError::Client)?;
        Ok(Self {
            client,
            slots: Arc::new(Semaphore::new(CONCURRENT)),
        })
    }

    /// Fetches the image a `mail-remote` request path names (its percent-encoded URL).
    pub async fn fetch(&self, request_path: &str) -> Result<Image, RemoteError> {
        let target = urls::decode(urls::request_path(request_path));
        let url = reqwest::Url::parse(&target).map_err(|_| RemoteError::BadUrl)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(RemoteError::BadUrl);
        }
        let _slot = self
            .slots
            .acquire()
            .await
            .map_err(|_| RemoteError::Failed)?;
        let mut resp = self
            .client
            .get(url)
            .header(reqwest::header::ACCEPT, "image/*")
            .send()
            .await
            .map_err(|_| RemoteError::Failed)?;
        if !resp.status().is_success() {
            return Err(RemoteError::Failed);
        }
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.split(';')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        if !content_type.starts_with("image/") {
            return Err(RemoteError::NotImage);
        }
        if resp.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
            return Err(RemoteError::TooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(|_| RemoteError::Failed)? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_BYTES {
                return Err(RemoteError::TooLarge);
            }
        }
        Ok(Image {
            content_type,
            bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::urls::UrlForm;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A local HTTP server that records each request's head and answers with `response`.
    async fn server(response: &'static [u8]) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = Arc::clone(&seen);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = vec![0; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let _ = sock.write_all(response).await;
                let _ = sock.shutdown().await;
            }
        });
        (base, seen)
    }

    fn path(url: &str) -> String {
        UrlForm::Custom
            .remote(url)
            .trim_start_matches("mail-remote://localhost")
            .to_string()
    }

    #[tokio::test]
    async fn images_are_fetched_without_cookies_or_referrer() {
        let (base, seen) = server(
            b"HTTP/1.1 200 OK\r\nContent-Type: image/gif\r\nContent-Length: 3\r\nSet-Cookie: t=1\r\nConnection: close\r\n\r\nGIF",
        )
        .await;
        let proxy = RemoteProxy::new().unwrap();
        let url = format!("{base}/pixel.gif?u=7");
        let img = proxy.fetch(&path(&url)).await.unwrap();
        assert_eq!(
            img,
            Image {
                content_type: "image/gif".into(),
                bytes: b"GIF".to_vec()
            }
        );
        // A second request must not send back the cookie the first one set.
        proxy.fetch(&path(&url)).await.unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        for head in &seen {
            let lower = head.to_ascii_lowercase();
            assert!(lower.starts_with("get /pixel.gif?u=7 http/1.1"), "{head}");
            assert!(!lower.contains("\r\ncookie:"), "{head}");
            assert!(!lower.contains("\r\nreferer:"), "{head}");
            assert!(lower.contains("\r\nuser-agent: mozilla/5.0\r\n"), "{head}");
        }
    }

    #[tokio::test]
    async fn only_images_and_only_http() {
        let (base, _) = server(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi",
        )
        .await;
        let proxy = RemoteProxy::new().unwrap();
        assert_eq!(
            proxy.fetch(&path(&format!("{base}/x"))).await,
            Err(RemoteError::NotImage)
        );
        assert_eq!(
            proxy.fetch(&path("file:///etc/passwd")).await,
            Err(RemoteError::BadUrl)
        );
        assert_eq!(
            proxy.fetch(&path("javascript:alert(1)")).await,
            Err(RemoteError::BadUrl)
        );
        let (big, _) = server(
            b"HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: 99999999\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert_eq!(
            proxy.fetch(&path(&format!("{big}/b.png"))).await,
            Err(RemoteError::TooLarge)
        );
    }
}
