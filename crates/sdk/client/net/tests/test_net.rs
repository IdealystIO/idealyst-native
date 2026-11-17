//! A test's hold/fail rules (`test-net`, driven by the robot's `net.*`
//! functions) apply to real requests: a held request reaches the server
//! only when released, a failed one never does. Own binary: the rules are
//! process-wide.

#![cfg(not(target_arch = "wasm32"))]

use std::future::Future;
use std::sync::mpsc;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

use net::{Client, Error};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn block_on<F: Future>(fut: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

/// A loopback server answering `200 hello`, reporting each request it reads.
fn serve() -> (String, mpsc::Receiver<()>, tokio::runtime::Runtime) {
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let (tx, rx) = mpsc::channel();
    let listener = rt.block_on(TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = listener.local_addr().unwrap();
    rt.spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut buf = [0u8; 1024];
                while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => head.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = tx.send(());
                let reply = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello";
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    (format!("http://{addr}"), rx, rt)
}

/// want_998196cd (CrewForge): search-survives-rebuild needs the search
/// request in flight while the filter bar rebuilds. A held request is
/// parked before the network and reaches the server only when released;
/// a failed one is answered without it.
#[test]
fn regression_a_held_request_waits_for_release_and_a_failed_one_never_leaves() {
    let (base, received, _rt) = serve();

    let rule = test_net::add_rule(test_net::Rule {
        url: "/_srv/*search".into(),
        method: None,
        action: test_net::Action::Hold,
        times: None,
    });
    let url = format!("{base}/_srv/project_search");
    let pending = std::thread::spawn(move || {
        block_on(async move {
            let r = Client::new().post(url).send().await?;
            Ok::<_, Error>((r.status(), r.text().await?))
        })
    });
    // Parked: listed, and the server has seen nothing.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let held = loop {
        let h = test_net::held();
        if !h.is_empty() {
            break h;
        }
        assert!(std::time::Instant::now() < deadline, "the request was never held");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(held[0].method, "POST");
    assert!(held[0].url.ends_with("/_srv/project_search"));
    assert!(received.recv_timeout(Duration::from_millis(200)).is_err(), "held: not sent");

    assert_eq!(test_net::release(test_net::Which::Rule(rule), None), 1);
    let (status, body) = pending.join().unwrap().expect("released: sent");
    assert_eq!((status, body.as_str()), (200, "hello"));
    received.recv_timeout(Duration::from_secs(5)).expect("the server got it once released");

    test_net::clear();
    test_net::add_rule(test_net::Rule {
        url: "/_srv/save".into(),
        method: Some("POST".into()),
        action: test_net::Action::Fail(test_net::Failure::Status { code: 503, body: "down".into() }),
        times: Some(1),
    });
    let save = format!("{base}/_srv/save");
    let r = block_on(Client::new().post(save.clone()).send()).expect("a faked response");
    assert_eq!(r.status(), 503);
    assert!(received.recv_timeout(Duration::from_millis(200)).is_err(), "failed: never sent");

    test_net::add_rule(test_net::Rule {
        url: "/_srv/save".into(),
        method: None,
        action: test_net::Action::Fail(test_net::Failure::Offline),
        times: Some(1),
    });
    assert!(matches!(block_on(Client::new().post(save.clone()).send()), Err(Error::Offline)));
    let ok = block_on(Client::new().post(save).send()).expect("rules used up: the network again");
    assert_eq!(ok.status(), 200);
    test_net::clear();
}
