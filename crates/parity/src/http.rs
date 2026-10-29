//! Servers: a recording proxy in front of the original and a replayer against the rewrite.
//!
//! `parity http record --listen ADDR` forwards every request to `[http] original_url` and keeps the exchange in
//! `corpus-http/NNNNNN/` (request.json, request.body, response.json, response.body). Point real clients (or a
//! load script) at ADDR. `parity http replay` sends the recorded requests, in order, to `candidate_url` and
//! compares status, headers and normalized bodies.

use crate::compare::{Divergence, Field};
use crate::normalize::{Normalizer, Target};
use crate::project::{HttpConfig, Layout};
use crate::report::{CaseResult, Report};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const HTTP_CORPUS: &str = "corpus-http";

/// Request bodies larger than this are refused by the recording proxy.
const MAX_BODY: u64 = 64 << 20;

/// Headers that describe the connection or the moment, never the behaviour.
const ALWAYS_IGNORED: &[&str] = &[
    "date",
    "server",
    "connection",
    "keep-alive",
    "transfer-encoding",
    "content-length",
    "host",
    "upgrade",
    "proxy-connection",
];

#[derive(Debug, Deserialize, Serialize)]
pub struct Request {
    pub method: String,
    /// Path and query.
    pub url: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub millis: u64,
}

fn corpus(layout: &Layout) -> PathBuf {
    layout.root.join(HTTP_CORPUS)
}

fn config(layout: &Layout) -> Result<HttpConfig> {
    layout
        .load_project()?
        .http
        .context("parity.toml has no [http] section")
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(120))
        .build()
}

/// Sends one request; 4xx and 5xx are answers like any other.
fn send(base: &str, req: &Request, body: &[u8]) -> Result<(Response, Vec<u8>)> {
    // Only origin-form targets: an absolute URL or `//host` would make the proxy fetch from elsewhere.
    if !req.url.starts_with('/') || req.url.starts_with("//") {
        bail!("request target {:?} is not a path", req.url);
    }
    let url = format!("{}{}", base.trim_end_matches('/'), req.url);
    let mut r = agent().request(&req.method, &url);
    for (k, v) in &req.headers {
        if !ALWAYS_IGNORED.contains(&k.to_ascii_lowercase().as_str()) {
            r = r.set(k, v);
        }
    }
    let started = Instant::now();
    let resp = match r.send_bytes(body) {
        Ok(resp) | Err(ureq::Error::Status(_, resp)) => resp,
        Err(e) => return Err(e).with_context(|| format!("{} {url}", req.method)),
    };
    let headers = resp
        .headers_names()
        .into_iter()
        .filter_map(|n| resp.header(&n).map(|v| (n.clone(), v.to_string())))
        .collect();
    let status = resp.status();
    let mut body = Vec::new();
    resp.into_reader().read_to_end(&mut body)?;
    Ok((
        Response {
            status,
            headers,
            millis: started.elapsed().as_millis() as u64,
        },
        body,
    ))
}

pub fn record(layout: &Layout, listen: &str) -> Result<()> {
    let cfg = config(layout)?;
    if cfg.original_url.is_empty() {
        bail!("[http] original_url is empty");
    }
    let dir = corpus(layout);
    std::fs::create_dir_all(&dir)?;
    let mut next = std::fs::read_dir(&dir)?.count();
    let server = tiny_http::Server::http(listen)
        .map_err(|e| anyhow::anyhow!("listening on {listen}: {e}"))?;
    println!(
        "recording: clients → http://{listen} → {}  (Ctrl+C to stop)",
        cfg.original_url
    );
    for mut incoming in server.incoming_requests() {
        let req = Request {
            method: incoming.method().as_str().to_string(),
            url: incoming.url().to_string(),
            headers: incoming
                .headers()
                .iter()
                .map(|h| {
                    (
                        h.field.as_str().as_str().to_string(),
                        h.value.as_str().to_string(),
                    )
                })
                .collect(),
        };
        // One bad request must never stop the proxy: every failure is an answer, then the next request.
        let mut body = Vec::new();
        let read = incoming
            .as_reader()
            .take(MAX_BODY + 1)
            .read_to_end(&mut body);
        if read.is_err() || body.len() as u64 > MAX_BODY {
            let _ = incoming.respond(
                tiny_http::Response::from_string("parity proxy: unreadable or too large body")
                    .with_status_code(400),
            );
            continue;
        }
        let answer = match send(&cfg.original_url, &req, &body) {
            Ok((resp, resp_body)) => {
                let case = dir.join(format!("{next:06}"));
                next += 1;
                let saved = (|| -> Result<()> {
                    std::fs::create_dir_all(&case)?;
                    std::fs::write(case.join("request.json"), serde_json::to_vec_pretty(&req)?)?;
                    std::fs::write(case.join("request.body"), &body)?;
                    std::fs::write(
                        case.join("response.json"),
                        serde_json::to_vec_pretty(&resp)?,
                    )?;
                    std::fs::write(case.join("response.body"), &resp_body)?;
                    Ok(())
                })();
                match saved {
                    Ok(()) => println!("{} {} → {}", req.method, req.url, resp.status),
                    Err(e) => eprintln!("{} {}: not recorded: {e:#}", req.method, req.url),
                }
                let mut out =
                    tiny_http::Response::from_data(resp_body).with_status_code(resp.status);
                for (k, v) in &resp.headers {
                    if !ALWAYS_IGNORED.contains(&k.to_ascii_lowercase().as_str())
                        && let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes())
                    {
                        out.add_header(h);
                    }
                }
                out
            }
            Err(e) => {
                eprintln!("{} {}: {e:#}", req.method, req.url);
                tiny_http::Response::from_string(format!("parity proxy: {e:#}"))
                    .with_status_code(502)
            }
        };
        let _ = incoming.respond(answer);
    }
    Ok(())
}

fn read_case(dir: &Path) -> Result<(Request, Vec<u8>, Response, Vec<u8>)> {
    let req: Request = serde_json::from_slice(&std::fs::read(dir.join("request.json"))?)?;
    let resp: Response = serde_json::from_slice(&std::fs::read(dir.join("response.json"))?)?;
    Ok((
        req,
        std::fs::read(dir.join("request.body"))?,
        resp,
        std::fs::read(dir.join("response.body"))?,
    ))
}

fn compare_exchange(
    expected: (&Response, &[u8]),
    actual: (&Response, &[u8]),
    ignore: &[String],
    norm: &Normalizer,
) -> Vec<Divergence> {
    let mut out = Vec::new();
    if expected.0.status != actual.0.status {
        out.push(Divergence {
            field: Field::Status,
            expected: expected.0.status.to_string(),
            actual: actual.0.status.to_string(),
        });
    }
    let headers = |r: &Response| -> BTreeMap<String, String> {
        r.headers
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
            .filter(|(k, _)| !ALWAYS_IGNORED.contains(&k.as_str()) && !ignore.contains(k))
            .collect()
    };
    let (he, ha) = (headers(expected.0), headers(actual.0));
    for name in he
        .keys()
        .chain(ha.keys())
        .collect::<std::collections::BTreeSet<_>>()
    {
        let e = he.get(name).map(|v| {
            String::from_utf8_lossy(&norm.apply(Target::Stdout, v.as_bytes())).into_owned()
        });
        let a = ha.get(name).map(|v| {
            String::from_utf8_lossy(&norm.apply(Target::Stdout, v.as_bytes())).into_owned()
        });
        if e != a {
            out.push(Divergence {
                field: Field::Header(name.clone()),
                expected: e.unwrap_or_else(|| crate::compare::ABSENT.into()),
                actual: a.unwrap_or_else(|| crate::compare::ABSENT.into()),
            });
        }
    }
    let (be, ba) = (
        norm.apply(Target::Stdout, expected.1),
        norm.apply(Target::Stdout, actual.1),
    );
    if be != ba {
        out.push(Divergence {
            field: Field::Body,
            expected: String::from_utf8_lossy(&be).into_owned(),
            actual: String::from_utf8_lossy(&ba).into_owned(),
        });
    }
    out
}

pub fn replay(layout: &Layout, candidate_url: Option<String>) -> Result<Report> {
    let project = layout.load_project()?;
    let cfg = project.http.unwrap_or_default();
    let base = candidate_url.unwrap_or(cfg.candidate_url.clone());
    if base.is_empty() {
        bail!("no candidate: set [http] candidate_url or pass --candidate-url");
    }
    let norm = Normalizer::new(&project.normalize)?;
    let ignore: Vec<String> = cfg
        .ignore_headers
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .collect();
    let mut names: Vec<String> = std::fs::read_dir(corpus(layout))
        .context("nothing recorded: run `parity http record` first")?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut cases = Vec::new();
    for name in names {
        let (req, body, expected, expected_body) = read_case(&corpus(layout).join(&name))?;
        let label = format!("{name} {} {}", req.method, req.url);
        let divergences = match send(&base, &req, &body) {
            Ok((resp, resp_body)) => {
                let d = compare_exchange(
                    (&expected, &expected_body),
                    (&resp, &resp_body),
                    &ignore,
                    &norm,
                );
                cases.push(CaseResult {
                    name: label,
                    divergences: d,
                    original_ms: expected.millis,
                    candidate_ms: resp.millis,
                });
                continue;
            }
            Err(e) => vec![Divergence {
                field: Field::Status,
                expected: expected.status.to_string(),
                actual: format!("{e:#}"),
            }],
        };
        cases.push(CaseResult {
            name: label,
            divergences,
            original_ms: expected.millis,
            candidate_ms: 0,
        });
    }
    Ok(Report {
        original: cfg.original_url,
        candidate: base,
        cases,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resp(status: u16, headers: &[(&str, &str)]) -> Response {
        Response {
            status,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            millis: 0,
        }
    }

    #[test]
    fn compares_status_headers_and_body_but_not_connection_noise() {
        let n = Normalizer::new(&[]).unwrap();
        let e = resp(
            200,
            &[
                ("Content-Type", "text/plain"),
                ("Date", "Mon"),
                ("X-Id", "1"),
            ],
        );
        let a = resp(
            201,
            &[
                ("content-type", "text/plain"),
                ("Date", "Tue"),
                ("X-Id", "2"),
            ],
        );
        let d = compare_exchange((&e, b"hi"), (&a, b"hi"), &[], &n);
        let fields: Vec<Field> = d.into_iter().map(|d| d.field).collect();
        assert_eq!(fields, [Field::Status, Field::Header("x-id".into())]);
        let d = compare_exchange((&e, b"hi"), (&e, b"ho"), &["x-id".into()], &n);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].field, Field::Body);
    }
}
