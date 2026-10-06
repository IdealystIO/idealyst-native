//! S3 over HTTP, signed in-process (AWS Signature Version 4): the console's
//! way to the bucket. The CLI uses the `aws` CLI instead, for its credential
//! chain (profiles, SSO); a server is configured with keys.
//!
//! Only what releases need: read an object (with its ETag), write one
//! (optionally only if it is absent or still at a given ETag), and list a
//! prefix. Path-style addressing (`<endpoint>/<bucket>/<key>`), which MinIO
//! requires and AWS accepts.

use anyhow::{anyhow, bail, Context, Result};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::{Expect, Written};

/// A bucket reached over HTTP.
#[derive(Clone, PartialEq, Eq)]
pub struct S3Http {
    /// `https://s3.us-east-1.amazonaws.com`, `http://localhost:9000`, …
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// The release location inside the bucket (no slashes at either end).
    pub prefix: String,
    pub access_key: String,
    pub secret_key: String,
    /// For temporary credentials.
    pub session_token: Option<String>,
}

impl std::fmt::Debug for S3Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the secret.
        write!(f, "S3Http({}/{}/{})", self.endpoint, self.bucket, self.prefix)
    }
}

impl S3Http {
    /// From the environment: the location as `s3://bucket/prefix`, and the
    /// variables the AWS tools read — `AWS_ACCESS_KEY_ID`,
    /// `AWS_SECRET_ACCESS_KEY`, optionally `AWS_SESSION_TOKEN`,
    /// `AWS_REGION` / `AWS_DEFAULT_REGION` (default `us-east-1`), and
    /// `AWS_ENDPOINT_URL` (default AWS's own for the region; MinIO's
    /// address for a local store).
    pub fn from_env(location: &str) -> Result<S3Http> {
        let rest = location.strip_prefix("s3://").ok_or_else(|| anyhow!("`{location}` isn't an s3:// location"))?;
        let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let region = var("AWS_REGION").or_else(|| var("AWS_DEFAULT_REGION")).unwrap_or_else(|| "us-east-1".into());
        Ok(S3Http {
            endpoint: var("AWS_ENDPOINT_URL").unwrap_or_else(|| format!("https://s3.{region}.amazonaws.com")).trim_end_matches('/').into(),
            region,
            bucket: bucket.into(),
            prefix: prefix.trim_matches('/').into(),
            access_key: var("AWS_ACCESS_KEY_ID").ok_or_else(|| anyhow!("AWS_ACCESS_KEY_ID is not set"))?,
            secret_key: var("AWS_SECRET_ACCESS_KEY").ok_or_else(|| anyhow!("AWS_SECRET_ACCESS_KEY is not set"))?,
            session_token: var("AWS_SESSION_TOKEN"),
        })
    }

    fn path(&self, key: &str) -> String {
        let key = if self.prefix.is_empty() { key.to_string() } else { format!("{}/{key}", self.prefix) };
        format!("/{}/{}", uri_encode(&self.bucket, false), uri_encode(&key, true))
    }

    fn host(&self) -> Result<&str> {
        self.endpoint
            .split_once("://")
            .map(|(_, rest)| rest.split('/').next().unwrap_or(rest))
            .ok_or_else(|| anyhow!("endpoint `{}` has no scheme", self.endpoint))
    }

    fn send(&self, method: reqwest::Method, key: &str, body: Vec<u8>, extra: &[(&str, String)]) -> Result<reqwest::blocking::Response> {
        self.send_to(method, &self.path(key), &[], body, extra)
    }

    /// A request to `path` (already encoded) with `query` parameters.
    fn send_to(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, &str)],
        body: Vec<u8>,
        extra: &[(&str, String)],
    ) -> Result<reqwest::blocking::Response> {
        let query = canonical_query(query);
        let payload = hex(&Sha256::digest(&body));
        let now = Stamp::now();
        let mut signed = vec![
            ("host".to_string(), self.host()?.to_string()),
            ("x-amz-content-sha256".to_string(), payload.clone()),
            ("x-amz-date".to_string(), now.long.clone()),
        ];
        if let Some(t) = &self.session_token {
            signed.push(("x-amz-security-token".to_string(), t.clone()));
        }
        let auth = authorization(method.as_str(), path, &query, &signed, &payload, &now, &self.region, &self.access_key, &self.secret_key);
        let client = reqwest::blocking::Client::new();
        let url = if query.is_empty() { format!("{}{path}", self.endpoint) } else { format!("{}{path}?{query}", self.endpoint) };
        let mut req = client.request(method, url).header("authorization", auth).body(body);
        for (k, v) in signed.iter().filter(|(k, _)| k != "host") {
            req = req.header(k.as_str(), v.as_str());
        }
        for (k, v) in extra {
            req = req.header(*k, v.as_str());
        }
        req.send().with_context(|| format!("reach {}", self.endpoint))
    }

    pub(crate) fn get(&self, key: &str) -> Result<Option<(Vec<u8>, String)>> {
        let resp = self.send(reqwest::Method::GET, key, Vec::new(), &[])?;
        match resp.status().as_u16() {
            404 => Ok(None),
            200 => {
                let etag = resp.headers().get("etag").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
                Ok(Some((resp.bytes()?.to_vec(), etag)))
            }
            s => bail!("read s3://{}/{}: HTTP {s}: {}", self.bucket, self.path(key), resp.text().unwrap_or_default()),
        }
    }

    pub(crate) fn put(&self, key: &str, bytes: &[u8], content_type: &str, cache: &str, expect: Option<&Expect>) -> Result<Written> {
        let mut extra = vec![("content-type", content_type.to_string()), ("cache-control", cache.to_string())];
        match expect {
            Some(Expect::Absent) => extra.push(("if-none-match", "*".into())),
            Some(Expect::Version(etag)) => extra.push(("if-match", etag.clone())),
            None => {}
        }
        let resp = self.send(reqwest::Method::PUT, key, bytes.to_vec(), &extra)?;
        match resp.status().as_u16() {
            200 => Ok(Written::Done),
            // 412: the precondition failed; 409: a concurrent conditional
            // write won (S3's ConditionalRequestConflict).
            412 | 409 => Ok(Written::Conflict),
            s => bail!("write s3://{}/{}: HTTP {s}: {}", self.bucket, self.path(key), resp.text().unwrap_or_default()),
        }
    }
}

impl S3Http {
    /// Every object under `prefix` (relative to the location): its path
    /// relative to the location, and when it was last written (seconds since
    /// the Unix epoch). ListObjectsV2, following continuation tokens.
    pub(crate) fn list(&self, prefix: &str) -> Result<Vec<(String, u64)>> {
        let full = if self.prefix.is_empty() { prefix.to_string() } else { format!("{}/{prefix}", self.prefix) };
        let strip = if self.prefix.is_empty() { String::new() } else { format!("{}/", self.prefix) };
        let path = format!("/{}", uri_encode(&self.bucket, false));
        let (mut out, mut token) = (Vec::new(), None::<String>);
        loop {
            let mut query = vec![("list-type", "2"), ("prefix", full.as_str())];
            if let Some(t) = &token {
                query.push(("continuation-token", t.as_str()));
            }
            let resp = self.send_to(reqwest::Method::GET, &path, &query, Vec::new(), &[])?;
            let status = resp.status().as_u16();
            let text = resp.text()?;
            if status != 200 {
                bail!("list s3://{}/{full}: HTTP {status}: {text}", self.bucket);
            }
            for object in xml_all(&text, "Contents") {
                let key = xml_one(object, "Key").map(xml_unescape).unwrap_or_default();
                let modified = xml_one(object, "LastModified").and_then(crate::iso_secs).unwrap_or(0);
                out.push((key.strip_prefix(&strip).unwrap_or(&key).to_string(), modified));
            }
            if xml_one(&text, "IsTruncated") != Some("true") {
                return Ok(out);
            }
            token = xml_one(&text, "NextContinuationToken").map(xml_unescape);
            if token.is_none() {
                return Ok(out);
            }
        }
    }
}

/// The text of the first `<tag>…</tag>` in `xml`.
fn xml_one<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&format!("</{tag}>"))?;
    Some(&xml[start..start + end])
}

/// The text of every `<tag>…</tag>` in `xml`.
fn xml_all<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find(&open) {
        let body = &rest[i + open.len()..];
        let Some(j) = body.find(&close) else { break };
        out.push(&body[..j]);
        rest = &body[j + close.len()..];
    }
    out
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// The canonical query string: parameters sorted, each name and value
/// URI-encoded (slashes too), joined with `&`.
fn canonical_query(query: &[(&str, &str)]) -> String {
    let mut pairs: Vec<(String, String)> = query.iter().map(|(k, v)| (uri_encode(k, false), uri_encode(v, false))).collect();
    pairs.sort();
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&")
}

/// A request time, in the two forms the signature uses.
pub(crate) struct Stamp {
    /// `20130524T000000Z`
    long: String,
    /// `20130524`
    short: String,
}

impl Stamp {
    fn now() -> Stamp {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Stamp::at(secs)
    }

    fn at(secs: u64) -> Stamp {
        // Howard Hinnant's days-to-civil.
        let days = (secs / 86_400) as i64 + 719_468;
        let era = days.div_euclid(146_097);
        let doe = days.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        let rem = secs % 86_400;
        let short = format!("{year:04}{month:02}{day:02}");
        Stamp { long: format!("{short}T{:02}{:02}{:02}Z", rem / 3_600, rem % 3_600 / 60, rem % 60), short }
    }
}

/// The `Authorization` header for a request: `headers` are the signed ones
/// (lower-case names), `payload` the hex SHA-256 of the body.
#[allow(clippy::too_many_arguments)]
fn authorization(
    method: &str,
    path: &str,
    query: &str,
    headers: &[(String, String)],
    payload: &str,
    at: &Stamp,
    region: &str,
    access_key: &str,
    secret_key: &str,
) -> String {
    let mut headers: Vec<&(String, String)> = headers.iter().collect();
    headers.sort_by(|a, b| a.0.cmp(&b.0));
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{}\n", v.trim())).collect();
    let signed_headers = headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>().join(";");
    let canonical = format!("{method}\n{path}\n{query}\n{canonical_headers}\n{signed_headers}\n{payload}");
    let scope = format!("{}/{region}/s3/aws4_request", at.short);
    let to_sign = format!("AWS4-HMAC-SHA256\n{}\n{scope}\n{}", at.long, hex(&Sha256::digest(canonical.as_bytes())));
    let key = [at.short.as_str(), region, "s3", "aws4_request"]
        .iter()
        .fold(format!("AWS4{secret_key}").into_bytes(), |k, part| hmac(&k, part.as_bytes()));
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    format!("AWS4-HMAC-SHA256 Credential={access_key}/{scope}, SignedHeaders={signed_headers}, Signature={signature}")
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// S3's URI encoding: everything but unreserved characters, and `/` too
/// unless `keep_slash` (an object key's separators stay).
fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AWS's own worked example (Signature Version 4, "GET Object"): the
    /// documented signature for this request is
    /// `f0e8bdb8…bdb41`. Pins the canonical request, scope and key
    /// derivation byte for byte.
    #[test]
    fn signs_the_aws_documented_example() {
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let at = Stamp { long: "20130524T000000Z".into(), short: "20130524".into() };
        let headers = vec![
            ("host".to_string(), "examplebucket.s3.amazonaws.com".to_string()),
            ("range".to_string(), "bytes=0-9".to_string()),
            ("x-amz-content-sha256".to_string(), empty.to_string()),
            ("x-amz-date".to_string(), "20130524T000000Z".to_string()),
        ];
        let auth = authorization(
            "GET",
            "/test.txt",
            "",
            &headers,
            empty,
            &at,
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    /// AWS's worked example "GET Bucket (List Objects)": a signed query
    /// string (`?max-keys=2&prefix=J`), documented signature `34b48302…`.
    #[test]
    fn signs_the_aws_documented_list_example() {
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let at = Stamp { long: "20130524T000000Z".into(), short: "20130524".into() };
        let headers = vec![
            ("host".to_string(), "examplebucket.s3.amazonaws.com".to_string()),
            ("x-amz-content-sha256".to_string(), empty.to_string()),
            ("x-amz-date".to_string(), "20130524T000000Z".to_string()),
        ];
        let query = canonical_query(&[("prefix", "J"), ("max-keys", "2")]);
        assert_eq!(query, "max-keys=2&prefix=J", "sorted");
        let auth = authorization(
            "GET",
            "/",
            &query,
            &headers,
            empty,
            &at,
            "us-east-1",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        );
        assert!(
            auth.ends_with("Signature=34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"),
            "{auth}"
        );
    }

    #[test]
    fn reads_a_listing() {
        let xml = "<ListBucketResult><IsTruncated>true</IsTruncated><Contents><Key>p/reported/a.json</Key><LastModified>2026-10-06T11:13:37.000Z</LastModified></Contents><Contents><Key>p/reported/b&amp;c.json</Key><LastModified>1970-01-01T00:00:10.000Z</LastModified></Contents><NextContinuationToken>t/1</NextContinuationToken></ListBucketResult>";
        let keys: Vec<String> = xml_all(xml, "Contents").iter().map(|c| xml_unescape(xml_one(c, "Key").unwrap())).collect();
        assert_eq!(keys, ["p/reported/a.json", "p/reported/b&c.json"]);
        assert_eq!(xml_one(xml, "NextContinuationToken"), Some("t/1"));
        assert_eq!(crate::iso_secs("1970-01-01T00:00:10.000Z"), Some(10));
    }

    #[test]
    fn stamps_and_paths() {
        let s = Stamp::at(1_369_353_600);
        assert_eq!((s.long.as_str(), s.short.as_str()), ("20130524T000000Z", "20130524"));
        assert_eq!(uri_encode("demo/bundles/a b+c.wasm", true), "demo/bundles/a%20b%2Bc.wasm");
    }
}
