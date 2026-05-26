//! Test harness: a local hyper server with a request-routing closure.

use std::convert::Infallible;
use std::net::SocketAddr;

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

/// One handled HTTP request, captured for assertions.
#[derive(Debug, Clone)]
pub struct Captured {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// A canned response the harness will return for a request.
#[derive(Debug, Clone)]
pub struct Canned {
    pub status: u16,
    pub headers: Vec<(&'static str, &'static str)>,
    pub body: Vec<u8>,
}

impl Canned {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: vec![],
            body: body.into(),
        }
    }
    pub fn json(body: &str) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type", "application/json")],
            body: body.as_bytes().to_vec(),
        }
    }
    pub fn status(code: u16) -> Self {
        Self {
            status: code,
            headers: vec![],
            body: format!("status {code}").into_bytes(),
        }
    }
}

/// Boots a server on a random port. Returns the base URL and a handle
/// to inspect the captured requests after.
pub async fn serve(reply: Canned) -> Server {
    serve_with(move |_| reply.clone()).await
}

pub async fn serve_with<F>(reply_fn: F) -> Server
where
    F: Fn(&Captured) -> Canned + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr: SocketAddr = listener.local_addr().expect("local_addr");

    let captured: std::sync::Arc<std::sync::Mutex<Vec<Captured>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let reply_fn = std::sync::Arc::new(reply_fn);

    let captured_for_task = captured.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let captured = captured_for_task.clone();
            let reply_fn = reply_fn.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req: Request<Incoming>| {
                    let captured = captured.clone();
                    let reply_fn = reply_fn.clone();
                    async move {
                        let (parts, body) = req.into_parts();
                        let body_bytes = body
                            .collect()
                            .await
                            .map(|c| c.to_bytes().to_vec())
                            .unwrap_or_default();
                        let captured_req = Captured {
                            method: parts.method.as_str().to_string(),
                            path: parts.uri.path().to_string(),
                            query: parts.uri.query().map(|s| s.to_string()),
                            headers: parts
                                .headers
                                .iter()
                                .map(|(n, v)| {
                                    (
                                        n.as_str().to_string(),
                                        v.to_str().unwrap_or("").to_string(),
                                    )
                                })
                                .collect(),
                            body: body_bytes,
                        };
                        let reply = (reply_fn)(&captured_req);
                        captured.lock().unwrap().push(captured_req);

                        let mut resp =
                            Response::builder().status(StatusCode::from_u16(reply.status).unwrap());
                        for (n, v) in &reply.headers {
                            resp = resp.header(*n, *v);
                        }
                        Ok::<_, Infallible>(
                            resp.body(Full::new(Bytes::from(reply.body))).unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });

    Server {
        base_url: format!("http://{addr}"),
        captured,
    }
}

pub struct Server {
    pub base_url: String,
    captured: std::sync::Arc<std::sync::Mutex<Vec<Captured>>>,
}

impl Server {
    pub fn captured(&self) -> Vec<Captured> {
        self.captured.lock().unwrap().clone()
    }
}
