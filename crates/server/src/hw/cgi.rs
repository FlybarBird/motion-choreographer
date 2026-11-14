//! HTTP GET with digest authentication, which is how the FR7's web interface
//! answers `command/inquiry.cgi` and serves preset pictures.

use md5::{Digest, Md5};
use std::time::Duration;

fn md5_hex(text: &str) -> String {
    Md5::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// The fields of a `WWW-Authenticate: Digest ...` header.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Challenge {
    pub realm: String,
    pub nonce: String,
    pub qop: Option<String>,
    pub opaque: Option<String>,
}

/// `name="value"` and `name=value` pairs of an authentication header.
pub fn auth_fields(header: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = header.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let name = rest[..eq].trim().trim_start_matches(',').trim().to_ascii_lowercase();
        rest = rest[eq + 1..].trim_start();
        let value;
        if let Some(quoted) = rest.strip_prefix('"') {
            let end = quoted.find('"').unwrap_or(quoted.len());
            value = quoted[..end].to_string();
            rest = quoted.get(end + 1..).unwrap_or("");
        } else {
            let end = rest.find(',').unwrap_or(rest.len());
            value = rest[..end].trim().to_string();
            rest = &rest[end..];
        }
        rest = rest.trim_start().trim_start_matches(',').trim_start();
        out.push((name, value));
    }
    out
}

impl Challenge {
    pub fn parse(header: &str) -> Option<Challenge> {
        let body = header.trim();
        let body = body.get(..6).filter(|p| p.eq_ignore_ascii_case("digest")).map(|_| &body[6..])?;
        let fields = auth_fields(body);
        let get = |name: &str| fields.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
        // A server may offer several protections; `auth` is the one answered here.
        let qop = get("qop").and_then(|q| q.split(',').map(str::trim).find(|q| *q == "auth").map(str::to_string));
        Some(Challenge { realm: get("realm")?, nonce: get("nonce")?, qop, opaque: get("opaque") })
    }
}

/// The `response` value of RFC 2617 for a GET of `uri`.
pub fn digest_response(user: &str, password: &str, c: &Challenge, uri: &str, nc: u32, cnonce: &str) -> String {
    let ha1 = md5_hex(&format!("{user}:{}:{password}", c.realm));
    let ha2 = md5_hex(&format!("GET:{uri}"));
    match &c.qop {
        Some(qop) => md5_hex(&format!("{ha1}:{}:{nc:08x}:{cnonce}:{qop}:{ha2}", c.nonce)),
        None => md5_hex(&format!("{ha1}:{}:{ha2}", c.nonce)),
    }
}

pub fn authorization(user: &str, password: &str, c: &Challenge, uri: &str, nc: u32, cnonce: &str) -> String {
    let response = digest_response(user, password, c, uri, nc, cnonce);
    let mut out = format!(
        "Digest username=\"{user}\", realm=\"{}\", nonce=\"{}\", uri=\"{uri}\", response=\"{response}\"",
        c.realm, c.nonce
    );
    if let Some(qop) = &c.qop {
        out.push_str(&format!(", qop={qop}, nc={nc:08x}, cnonce=\"{cnonce}\""));
    }
    if let Some(opaque) = &c.opaque {
        out.push_str(&format!(", opaque=\"{opaque}\""));
    }
    out
}

pub struct Cgi {
    agent: ureq::Agent,
    base: String,
    user: String,
    password: String,
    challenge: Option<Challenge>,
    nc: u32,
}

impl Cgi {
    /// `host` is `name` or `name:port`.
    pub fn new(host: &str, user: &str, password: &str, timeout: Duration) -> Cgi {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(timeout))
            .timeout_global(Some(timeout))
            .build();
        Cgi {
            agent: ureq::Agent::new_with_config(config),
            base: format!("http://{host}"),
            user: user.to_string(),
            password: password.to_string(),
            challenge: None,
            nc: 0,
        }
    }

    fn cnonce(&self) -> String {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        md5_hex(&format!("{nanos}:{}", self.nc))[..16].to_string()
    }

    fn once(&mut self, uri: &str) -> Result<(u16, Option<String>, Vec<u8>), String> {
        let mut req = self.agent.get(format!("{}{uri}", self.base));
        if let Some(c) = &self.challenge {
            self.nc += 1;
            req = req.header("Authorization", authorization(&self.user, &self.password, c, uri, self.nc, &self.cnonce()));
        }
        let mut resp = req.call().map_err(|e| format!("no answer from {}: {e}", self.base))?;
        let code = resp.status().as_u16();
        let offer = resp.headers().get("www-authenticate").and_then(|v| v.to_str().ok()).map(str::to_string);
        let body = resp.body_mut().with_config().limit(16 * 1024 * 1024).read_to_vec().map_err(|e| e.to_string())?;
        Ok((code, offer, body))
    }

    /// GET `uri` (a path starting with `/`), answering a digest challenge when one comes back.
    pub fn get(&mut self, uri: &str) -> Result<Vec<u8>, String> {
        let (mut code, mut offer, mut body) = self.once(uri)?;
        if code == 401 {
            // The first 401 carries the challenge; a later one means the nonce went stale.
            let Some(challenge) = offer.as_deref().and_then(Challenge::parse) else {
                return Err(format!("{} wants a sign-in this program does not speak", self.base));
            };
            self.challenge = Some(challenge);
            self.nc = 0;
            (code, offer, body) = self.once(uri)?;
            let _ = offer;
            if code == 401 {
                return Err(format!("{} refused the user name or password", self.base));
            }
        }
        if (200..300).contains(&code) {
            Ok(body)
        } else {
            Err(format!("{}{uri} answered {code}", self.base))
        }
    }

    pub fn get_text(&mut self, uri: &str) -> Result<String, String> {
        self.get(uri).map(|b| String::from_utf8_lossy(&b).to_string())
    }
}

/// `key=value` pairs of a CGI answer, separated by `&` or line ends.
pub fn fields(text: &str) -> Vec<(String, String)> {
    text.split(['&', '\n', '\r'])
        .filter_map(|part| part.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

pub fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_response_matches_the_rfc_2617_example() {
        let header = "Digest realm=\"testrealm@host.com\", qop=\"auth,auth-int\", \
                      nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\", opaque=\"5ccc069c403ebaf9f0171e9517f40e41\"";
        let c = Challenge::parse(header).unwrap();
        assert_eq!(c.realm, "testrealm@host.com");
        assert_eq!(c.qop.as_deref(), Some("auth"));
        let response = digest_response("Mufasa", "Circle Of Life", &c, "/dir/index.html", 1, "0a4f113b");
        assert_eq!(response, "6629fae49393a05397450978507c4ef1");
        let sent = authorization("Mufasa", "Circle Of Life", &c, "/dir/index.html", 1, "0a4f113b");
        let fields = auth_fields(sent.strip_prefix("Digest").unwrap());
        let get = |n: &str| fields.iter().find(|(k, _)| k == n).map(|(_, v)| v.as_str());
        assert_eq!(get("response"), Some("6629fae49393a05397450978507c4ef1"));
        assert_eq!(get("nc"), Some("00000001"));
        assert_eq!(get("uri"), Some("/dir/index.html"));
        assert_eq!(get("opaque"), Some("5ccc069c403ebaf9f0171e9517f40e41"));
    }

    #[test]
    fn a_challenge_without_qop_uses_the_older_form() {
        let c = Challenge::parse("Digest realm=\"cam\", nonce=\"abc\"").unwrap();
        assert_eq!(c.qop, None);
        let ha1 = md5_hex("admin:cam:pw");
        let ha2 = md5_hex("GET:/x");
        assert_eq!(digest_response("admin", "pw", &c, "/x", 1, "ignored"), md5_hex(&format!("{ha1}:abc:{ha2}")));
        assert_eq!(Challenge::parse("Basic realm=\"cam\""), None);
    }

    #[test]
    fn cgi_answers_split_into_fields() {
        let f = fields("PresetNum=2&PresetName=1,Wide,2,Lectern\r\n");
        assert_eq!(field(&f, "presetname"), Some("1,Wide,2,Lectern"));
        assert_eq!(field(&f, "PresetNum"), Some("2"));
        assert_eq!(field(&f, "Missing"), None);
    }
}
