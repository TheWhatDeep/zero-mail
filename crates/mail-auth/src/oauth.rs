//! Google sign-in (section 5): the user's own Desktop-app client, PKCE, and a loopback
//! redirect to `127.0.0.1` on a random port, with the scope `https://mail.google.com/`.
//!
//! - [`begin`] binds the loopback listener and builds the consent URL the app opens in the
//!   system browser.
//! - [`finish`] waits for the browser's redirect, checks its `state`, and exchanges the code
//!   for tokens at Google's token endpoint.
//! - [`refresh`] trades the refresh token for a new access token.
//!
//! The client secret Google shows for a Desktop client is required by its token endpoint
//! and is sent nowhere else (SPEC_GAPS G70). Access tokens are returned to the caller and
//! never persisted here; the refresh token is the caller's to keep in the keychain.

use std::future::Future;
use std::net::Ipv4Addr;
use std::pin::Pin;
use std::time::Duration;

use mail_secrets::Secret;
use oauth2::basic::{BasicClient, BasicErrorResponse, BasicErrorResponseType};
use oauth2::{
    AsyncHttpClient, AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    EndpointNotSet, EndpointSet, HttpRequest, HttpResponse, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, RefreshToken, RequestTokenError, Scope, TokenResponse, TokenUrl,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;

use crate::AuthError;

/// Google's authorization endpoint.
pub const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
/// Google's token endpoint.
pub const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
/// Full IMAP and SMTP access (section 5).
pub const SCOPE: &str = "https://mail.google.com/";
/// How long the loopback listener waits for the browser.
pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Access tokens are refreshed this long before they expire (section 5, Tokens).
pub const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);
/// Assumed lifetime when the token endpoint gives none (Google's is one hour).
const DEFAULT_LIFETIME: Duration = Duration::from_secs(60 * 60);
/// Time to read one request on the loopback listener.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest request head read on the loopback listener.
const MAX_REQUEST: usize = 16 * 1024;

/// A form POST that failed before any HTTP answer arrived.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct HttpFailure(pub String);

/// An HTTP answer: the status and the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpReply {
    pub status: u16,
    pub body: Vec<u8>,
}

/// POSTs a form to the token endpoint. The app uses [`crate::HttpClient`]; tests use a
/// fake token endpoint.
pub trait HttpPost: Send + Sync {
    /// `body` is `application/x-www-form-urlencoded`; the answer is expected as JSON.
    fn post_form(
        &self,
        url: &str,
        body: Vec<u8>,
    ) -> impl Future<Output = Result<HttpReply, HttpFailure>> + Send;
}

/// The user's own OAuth client (section 5, Bring-your-own OAuth client).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleClient {
    pub client_id: String,
    pub client_secret: Secret,
}

/// Tokens from one exchange. `refresh` is `None` when the endpoint kept the old one.
#[derive(Debug, Clone)]
pub struct Tokens {
    pub access: Secret,
    pub expires_at: Instant,
    pub refresh: Option<Secret>,
}

/// Whether an access token is too close to its expiry to use (section 5: refresh 5 minutes
/// before expiry).
pub fn needs_refresh(expires_at: Instant, now: Instant) -> bool {
    expires_at.saturating_duration_since(now) <= REFRESH_MARGIN
}

type Client = BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

fn client(c: &GoogleClient) -> Result<Client, AuthError> {
    let bad = |_| AuthError::OAuth("the Google endpoints are not valid URLs".into());
    Ok(
        BasicClient::new(ClientId::new(c.client_id.trim().to_string()))
            .set_client_secret(ClientSecret::new(
                c.client_secret.expose().trim().to_string(),
            ))
            .set_auth_type(AuthType::RequestBody)
            .set_auth_uri(AuthUrl::new(AUTH_URL.to_string()).map_err(bad)?)
            .set_token_uri(TokenUrl::new(TOKEN_URL.to_string()).map_err(bad)?),
    )
}

/// A sign-in waiting for the browser.
#[derive(Debug)]
pub struct SignIn {
    listener: TcpListener,
    state: CsrfToken,
    verifier: PkceCodeVerifier,
    redirect: RedirectUrl,
    url: String,
}

impl SignIn {
    /// The consent URL to open in the system browser.
    pub fn url(&self) -> &str {
        &self.url
    }
}

/// Binds the loopback listener on a random port and builds the consent URL. `login_hint`
/// preselects the account's address in Google's account chooser.
pub async fn begin(c: &GoogleClient, login_hint: &str) -> Result<SignIn, AuthError> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| AuthError::OAuth(format!("loopback listener: {}", e.kind())))?;
    let port = listener
        .local_addr()
        .map_err(|e| AuthError::OAuth(format!("loopback listener: {}", e.kind())))?
        .port();
    let redirect = RedirectUrl::new(format!("http://127.0.0.1:{port}"))
        .map_err(|_| AuthError::OAuth("loopback address".into()))?;
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, state) = client(c)?
        .set_redirect_uri(redirect.clone())
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new(SCOPE.to_string()))
        .set_pkce_challenge(challenge)
        // A refresh token comes only with offline access, and again on every consent.
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent")
        .add_extra_param("login_hint", login_hint.trim())
        .url();
    Ok(SignIn {
        listener,
        state,
        verifier,
        redirect,
        url: url.to_string(),
    })
}

/// Waits up to `timeout` for the browser's redirect, then exchanges the code. The listener
/// closes when this returns, whatever the outcome.
pub async fn finish(
    sign_in: SignIn,
    c: &GoogleClient,
    http: &(impl HttpPost + Clone + 'static),
    timeout: Duration,
) -> Result<Tokens, AuthError> {
    let code = tokio::time::timeout(
        timeout,
        wait_for_code(&sign_in.listener, sign_in.state.secret()),
    )
    .await
    .map_err(|_| AuthError::OAuth("the browser sign-in timed out".into()))??;
    drop(sign_in.listener);
    let reply = client(c)?
        .set_redirect_uri(sign_in.redirect)
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(sign_in.verifier)
        .request_async(&Adapter(http.clone()))
        .await
        .map_err(token_error)?;
    let tokens = tokens(&reply);
    if tokens.refresh.is_none() {
        return Err(AuthError::OAuth("Google returned no refresh token".into()));
    }
    Ok(tokens)
}

/// A new access token for the refresh token. `invalid_grant` (revoked or expired) and a
/// refused client are [`AuthError::Rejected`]; trouble reaching Google is
/// [`AuthError::Network`].
pub async fn refresh(
    c: &GoogleClient,
    http: &(impl HttpPost + Clone + 'static),
    refresh_token: &Secret,
) -> Result<Tokens, AuthError> {
    let token = RefreshToken::new(refresh_token.expose().to_string());
    let reply = client(c)?
        .exchange_refresh_token(&token)
        .request_async(&Adapter(http.clone()))
        .await
        .map_err(token_error)?;
    Ok(tokens(&reply))
}

fn tokens(reply: &oauth2::basic::BasicTokenResponse) -> Tokens {
    Tokens {
        access: Secret::new(reply.access_token().secret().clone()),
        expires_at: Instant::now() + reply.expires_in().unwrap_or(DEFAULT_LIFETIME),
        refresh: reply
            .refresh_token()
            .map(|r| Secret::new(r.secret().clone())),
    }
}

/// Lets `oauth2` send its requests through an [`HttpPost`].
struct Adapter<H>(H);

type BoxedReply<'c> = Pin<Box<dyn Future<Output = Result<HttpResponse, HttpFailure>> + Send + 'c>>;

impl<'c, H: HttpPost + 'static> AsyncHttpClient<'c> for Adapter<H> {
    type Error = HttpFailure;
    type Future = BoxedReply<'c>;

    fn call(&'c self, request: HttpRequest) -> Self::Future {
        Box::pin(post(&self.0, request))
    }
}

async fn post<H: HttpPost>(http: &H, req: HttpRequest) -> Result<HttpResponse, HttpFailure> {
    let url = req.uri().to_string();
    let reply = http.post_form(&url, req.into_body()).await?;
    oauth2::http::Response::builder()
        .status(reply.status)
        .body(reply.body)
        .map_err(|e| HttpFailure(e.to_string()))
}

fn token_error(e: RequestTokenError<HttpFailure, BasicErrorResponse>) -> AuthError {
    match e {
        RequestTokenError::ServerResponse(r) => match r.error() {
            BasicErrorResponseType::Extension(x) if x == "temporarily_unavailable" => {
                AuthError::Network(x.clone())
            }
            other => AuthError::Rejected(other.as_ref().to_string()),
        },
        RequestTokenError::Request(e) => AuthError::Network(e.0),
        // A 5xx page or a garbled answer: try again later.
        RequestTokenError::Parse(..) | RequestTokenError::Other(_) => {
            AuthError::Network("unexpected answer from Google's token endpoint".into())
        }
    }
}

/// What one loopback request said.
#[derive(Debug, PartialEq, Eq)]
enum Redirect {
    Code(String),
    Denied(String),
    /// Not our redirect (a wrong `state`, a favicon request): answered and ignored.
    Ignore,
}

const DONE_PAGE: &str = "Signed in. You can close this tab and return to Zero Mail.";
const DENIED_PAGE: &str =
    "Sign-in did not complete. You can close this tab and try again in Zero Mail.";

/// Accepts loopback connections until one carries our `state` with a code or an error.
async fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, AuthError> {
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .map_err(|e| AuthError::OAuth(format!("loopback listener: {}", e.kind())))?;
        match tokio::time::timeout(REQUEST_TIMEOUT, one_request(stream, state)).await {
            Ok(Redirect::Code(code)) => return Ok(code),
            Ok(Redirect::Denied(why)) => {
                return Err(AuthError::OAuth(format!("Google sign-in ended: {why}")))
            }
            Ok(Redirect::Ignore) | Err(_) => {}
        }
    }
}

async fn one_request(mut stream: TcpStream, state: &str) -> Redirect {
    let mut head = Vec::new();
    let mut buf = [0u8; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < MAX_REQUEST {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let line = String::from_utf8_lossy(&head);
    let line = line.lines().next().unwrap_or_default();
    let outcome = parse_redirect(line, state);
    let (status, text) = match &outcome {
        Redirect::Code(_) => ("200 OK", DONE_PAGE),
        Redirect::Denied(_) => ("200 OK", DENIED_PAGE),
        Redirect::Ignore => ("404 Not Found", ""),
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Zero Mail</title></head>\
         <body><p>{text}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Security-Policy: default-src 'none'\r\nCache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
    outcome
}

/// Reads `GET /?state=…&code=…` (or `error=…`). Anything without our `state` is ignored.
fn parse_redirect(request_line: &str, state: &str) -> Redirect {
    let mut parts = request_line.split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Redirect::Ignore;
    };
    let Ok(url) = oauth2::url::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Redirect::Ignore;
    };
    let (mut got_state, mut code, mut error) = (None, None, None);
    for (k, v) in url.query_pairs() {
        match k.as_ref() {
            "state" => got_state = Some(v.into_owned()),
            "code" => code = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }
    if got_state.as_deref() != Some(state) {
        return Redirect::Ignore;
    }
    match (code, error) {
        (_, Some(e)) => Redirect::Denied(e),
        (Some(c), None) if !c.is_empty() => Redirect::Code(c),
        _ => Redirect::Denied("no code".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// A scripted token endpoint that records every form it receives.
    #[derive(Debug, Default, Clone)]
    struct Endpoint {
        replies: Arc<Mutex<Vec<Result<HttpReply, HttpFailure>>>>,
        seen: Arc<Mutex<Vec<(String, String)>>>,
    }

    impl Endpoint {
        fn reply(&self, status: u16, json: &str) {
            self.replies.lock().unwrap().push(Ok(HttpReply {
                status,
                body: json.as_bytes().to_vec(),
            }));
        }
    }

    impl HttpPost for Endpoint {
        async fn post_form(&self, url: &str, body: Vec<u8>) -> Result<HttpReply, HttpFailure> {
            self.seen
                .lock()
                .unwrap()
                .push((url.to_string(), String::from_utf8(body).unwrap()));
            let mut r = self.replies.lock().unwrap();
            if r.is_empty() {
                Err(HttpFailure("unreachable".into()))
            } else {
                r.remove(0)
            }
        }
    }

    fn google() -> GoogleClient {
        GoogleClient {
            client_id: "id-1.apps.googleusercontent.com".into(),
            client_secret: Secret::new("s3cret"),
        }
    }

    fn param(url: &str, name: &str) -> Option<String> {
        oauth2::url::Url::parse(url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.into_owned())
    }

    fn form(body: &str) -> Vec<(String, String)> {
        oauth2::url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect()
    }

    /// Plays the browser: one GET on the loopback redirect.
    async fn browse(redirect: &str, query: &str) -> String {
        let addr = redirect.trim_start_matches("http://").trim_end_matches('/');
        let mut s = TcpStream::connect(addr).await.unwrap();
        s.write_all(format!("GET /?{query} HTTP/1.1\r\nHost: {addr}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn consent_url_has_pkce_loopback_scope_and_offline_access() {
        let sign_in = begin(&google(), "me@example.com").await.unwrap();
        let url = sign_in.url();
        assert!(url.starts_with(AUTH_URL), "{url}");
        assert_eq!(param(url, "scope").unwrap(), SCOPE);
        assert_eq!(param(url, "code_challenge_method").unwrap(), "S256");
        assert_eq!(param(url, "code_challenge").unwrap().len(), 43);
        assert_eq!(param(url, "access_type").unwrap(), "offline");
        assert_eq!(param(url, "prompt").unwrap(), "consent");
        assert_eq!(param(url, "login_hint").unwrap(), "me@example.com");
        assert_eq!(param(url, "response_type").unwrap(), "code");
        assert_eq!(
            param(url, "client_id").unwrap(),
            "id-1.apps.googleusercontent.com"
        );
        let redirect = param(url, "redirect_uri").unwrap();
        let port: u16 = redirect
            .strip_prefix("http://127.0.0.1:")
            .unwrap()
            .trim_end_matches('/')
            .parse()
            .unwrap();
        assert_ne!(port, 0);
        assert!(param(url, "state").unwrap().len() >= 16);
        // The secret never appears in the browser URL.
        assert!(!url.contains("s3cret"));
    }

    #[tokio::test]
    async fn code_exchange_sends_verifier_and_secret_only_to_the_token_endpoint() {
        let endpoint = Endpoint::default();
        endpoint.reply(
            200,
            r#"{"access_token":"at-1","expires_in":3599,"refresh_token":"rt-1","token_type":"Bearer","scope":"https://mail.google.com/"}"#,
        );
        let sign_in = begin(&google(), "me@example.com").await.unwrap();
        let url = sign_in.url().to_string();
        let redirect = param(&url, "redirect_uri").unwrap();
        let state = param(&url, "state").unwrap();
        let ep = endpoint.clone();
        let finished =
            tokio::spawn(async move { finish(sign_in, &google(), &ep, SIGN_IN_TIMEOUT).await });
        // A stray request and a forged state are answered and ignored.
        assert!(browse(&redirect, "x=1").await.starts_with("HTTP/1.1 404"));
        assert!(browse(&redirect, "state=forged&code=evil")
            .await
            .starts_with("HTTP/1.1 404"));
        let page = browse(&redirect, &format!("state={state}&code=c0de&scope=x")).await;
        assert!(page.starts_with("HTTP/1.1 200"));
        assert!(page.contains("Content-Security-Policy: default-src 'none'"));
        let tokens = finished.await.unwrap().unwrap();
        assert_eq!(tokens.access.expose(), "at-1");
        assert_eq!(tokens.refresh.unwrap().expose(), "rt-1");
        assert!(tokens.expires_at > Instant::now() + Duration::from_secs(3500));
        let seen = endpoint.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, TOKEN_URL);
        let f = form(&seen[0].1);
        let get = |k: &str| f.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("grant_type"), Some("authorization_code"));
        assert_eq!(get("code"), Some("c0de"));
        assert_eq!(get("client_secret"), Some("s3cret"));
        assert_eq!(get("redirect_uri"), Some(redirect.as_str()));
        assert_eq!(get("code_verifier").map(str::len), Some(43));
        // The listener is closed afterwards.
        let addr = redirect.trim_start_matches("http://").trim_end_matches('/');
        assert!(TcpStream::connect(addr).await.is_err());
    }

    #[tokio::test]
    async fn a_denied_consent_ends_the_sign_in() {
        let sign_in = begin(&google(), "me@example.com").await.unwrap();
        let url = sign_in.url().to_string();
        let redirect = param(&url, "redirect_uri").unwrap();
        let state = param(&url, "state").unwrap();
        let finished = tokio::spawn(async move {
            finish(sign_in, &google(), &Endpoint::default(), SIGN_IN_TIMEOUT).await
        });
        let page = browse(&redirect, &format!("state={state}&error=access_denied")).await;
        assert!(page.contains("did not complete"));
        let err = finished.await.unwrap().unwrap_err();
        assert!(matches!(err, AuthError::OAuth(ref m) if m.contains("access_denied")));
    }

    #[tokio::test]
    async fn an_unanswered_sign_in_times_out() {
        let sign_in = begin(&google(), "me@example.com").await.unwrap();
        let err = finish(
            sign_in,
            &google(),
            &Endpoint::default(),
            Duration::from_millis(50),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, AuthError::OAuth(ref m) if m.contains("timed out")));
    }

    #[tokio::test]
    async fn refresh_errors_are_classified() {
        let endpoint = Endpoint::default();
        endpoint.reply(
            200,
            r#"{"access_token":"at-2","expires_in":3599,"token_type":"Bearer"}"#,
        );
        endpoint.reply(
            400,
            r#"{"error":"invalid_grant","error_description":"Token has been expired or revoked."}"#,
        );
        endpoint.reply(503, "<html>busy</html>");
        let rt = Secret::new("rt-1");
        let fresh = refresh(&google(), &endpoint, &rt).await.unwrap();
        assert_eq!(fresh.access.expose(), "at-2");
        assert!(fresh.refresh.is_none());
        assert_eq!(
            refresh(&google(), &endpoint, &rt).await.unwrap_err(),
            AuthError::Rejected("invalid_grant".into())
        );
        assert!(matches!(
            refresh(&google(), &endpoint, &rt).await.unwrap_err(),
            AuthError::Network(_)
        ));
        // Nothing scripted: the endpoint cannot be reached.
        assert!(matches!(
            refresh(&google(), &endpoint, &rt).await.unwrap_err(),
            AuthError::Network(_)
        ));
        let f = form(&endpoint.seen.lock().unwrap()[0].1);
        assert!(f.contains(&("grant_type".into(), "refresh_token".into())));
        assert!(f.contains(&("refresh_token".into(), "rt-1".into())));
        assert!(f.contains(&("client_secret".into(), "s3cret".into())));
    }

    #[test]
    fn tokens_refresh_five_minutes_before_expiry() {
        let now = Instant::now();
        assert!(!needs_refresh(now + Duration::from_secs(301), now));
        assert!(needs_refresh(now + Duration::from_secs(300), now));
        assert!(needs_refresh(now, now + Duration::from_secs(1)));
    }

    #[test]
    fn redirect_lines() {
        assert_eq!(
            parse_redirect("GET /?state=s&code=4%2F0A HTTP/1.1", "s"),
            Redirect::Code("4/0A".into())
        );
        assert_eq!(
            parse_redirect("POST /?state=s&code=c HTTP/1.1", "s"),
            Redirect::Ignore
        );
        assert_eq!(
            parse_redirect("GET /favicon.ico HTTP/1.1", "s"),
            Redirect::Ignore
        );
        assert_eq!(parse_redirect("", "s"), Redirect::Ignore);
        assert_eq!(
            parse_redirect("GET /?state=s HTTP/1.1", "s"),
            Redirect::Denied("no code".into())
        );
    }
}
