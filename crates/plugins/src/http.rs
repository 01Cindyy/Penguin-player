//! Web requests for plugins the user allowed to make them (`mp.http`). Only HTTPS to the
//! sites a plugin names in `plugin.json`. Each request runs on its own thread, and the
//! host thread hands the result to the plugin's callback.

use std::time::Duration;

use mp_core::{APP_NAME, APP_WEBSITE, VERSION};
use ureq::http::Uri;

/// Largest response body a plugin receives; larger responses fail.
pub(crate) const MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;
/// Longest a request may take, from connecting to the last byte.
const TIMEOUT: Duration = Duration::from_secs(20);

/// Headers the app sets itself: the user agent names the app, and the host is the URL's.
const RESERVED_HEADERS: [&str; 2] = ["host", "user-agent"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Makes web requests: [`WebClient`] in the app, canned answers in tests.
pub trait HttpClient: Send + Sync {
    /// `GET`s `request.url`, which [`check_url`] has allowed. Error statuses such as 404
    /// are responses; errors are short messages for the plugin.
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, String>;
}

/// The real internet, over HTTPS with built-in root certificates.
pub struct WebClient {
    agent: ureq::Agent,
}

impl Default for WebClient {
    fn default() -> Self {
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            // A redirect could lead to a site the plugin may not use.
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(TIMEOUT))
            .user_agent(format!("{APP_NAME}/{VERSION} ({APP_WEBSITE})"))
            .build()
            .into();
        Self { agent }
    }
}

impl HttpClient for WebClient {
    fn get(&self, request: &HttpRequest) -> Result<HttpResponse, String> {
        let mut call = self.agent.get(&request.url);
        for (name, value) in &request.headers {
            call = call.header(name, value);
        }
        let mut response = call.call().map_err(|err| err.to_string())?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()
            .map_err(|err| match err {
                ureq::Error::BodyExceedsLimit(_) => format!(
                    "the response is larger than {} MiB",
                    MAX_BODY_BYTES / 1024 / 1024
                ),
                err => err.to_string(),
            })?;
        Ok(HttpResponse { status, body })
    }
}

/// Answers every request with an error. Plugins get it where nothing may go online,
/// such as in tests.
pub struct Offline;

impl HttpClient for Offline {
    fn get(&self, _request: &HttpRequest) -> Result<HttpResponse, String> {
        Err("offline".to_owned())
    }
}

/// Checks that `url` is an HTTPS address on one of the `allowed` hosts, with no port
/// or user name that could lead elsewhere.
pub(crate) fn check_url(url: &str, allowed: &[String]) -> Result<(), String> {
    let uri: Uri = url
        .parse()
        .map_err(|_| format!("'{url}' is not a valid web address"))?;
    if uri.scheme_str() != Some("https") {
        return Err(format!("'{url}' must start with https://"));
    }
    let authority = uri.authority().map(|authority| authority.as_str());
    let host = uri.host().unwrap_or_default();
    if authority != Some(host) {
        return Err(format!(
            "'{url}' may not name a port or user, only the site"
        ));
    }
    if !allowed
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(host))
    {
        return Err(format!(
            "the plugin may not connect to {host}; plugin.json allows: {}",
            if allowed.is_empty() {
                "no sites".to_owned()
            } else {
                allowed.join(", ")
            }
        ));
    }
    Ok(())
}

/// Whether plugins may set header `name` on their requests.
pub(crate) fn is_reserved_header(name: &str) -> bool {
    RESERVED_HEADERS
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
}

/// `url` with `query` added, its names and values percent-encoded.
pub(crate) fn with_query(url: &str, query: &[(String, String)]) -> String {
    if query.is_empty() {
        return url.to_owned();
    }
    let pairs: Vec<String> = query
        .iter()
        .map(|(name, value)| format!("{}={}", encode(name), encode(value)))
        .collect();
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}{}", pairs.join("&"))
}

fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_to_allowed_sites() {
        let allowed = ["lrclib.net".to_owned()];
        assert!(check_url("https://lrclib.net/api/search?q=a", &allowed).is_ok());
        assert!(check_url("https://LRCLIB.net/", &allowed).is_ok());
        for url in [
            "http://lrclib.net/api",
            "https://lrclib.net:8443/api",
            "https://user@lrclib.net/api",
            "https://lrclib.net.evil.com/api",
            "https://evil.com/?lrclib.net",
            "https://sub.lrclib.net/",
            "ftp://lrclib.net/",
            "lrclib.net/api",
            "not a url",
        ] {
            assert!(check_url(url, &allowed).is_err(), "{url}");
        }
        let refused = check_url("https://lrclib.net/", &[]).unwrap_err();
        assert!(refused.contains("no sites"), "{refused}");
    }

    #[test]
    fn queries_are_encoded() {
        let query = [
            ("track_name".to_owned(), "Rain & Tin/2 (Live)".to_owned()),
            ("artist_name".to_owned(), "Kōji".to_owned()),
        ];
        assert_eq!(
            with_query("https://lrclib.net/api/search", &query),
            "https://lrclib.net/api/search?track_name=Rain%20%26%20Tin%2F2%20%28Live%29\
             &artist_name=K%C5%8Dji"
        );
        assert_eq!(
            with_query("https://a.org/?x=1", &query[..1]),
            "https://a.org/?x=1&track_name=Rain%20%26%20Tin%2F2%20%28Live%29"
        );
        assert_eq!(with_query("https://a.org/", &[]), "https://a.org/");
    }

    #[test]
    fn the_app_sets_some_headers_itself() {
        assert!(is_reserved_header("User-Agent"));
        assert!(is_reserved_header("host"));
        assert!(!is_reserved_header("Accept"));
    }
}
