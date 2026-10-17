//! Net page — built via the `docs!` macro.

use docs_macro::docs;
#[allow(unused_imports)]
use crate::shell::{CodeBlock, PageHeader, CodeBlockProps, PageHeaderProps};
#[allow(unused_imports)]
use idea_ui::{Typography, Card, Stack};

docs! {
    slug = "net",
    title = "Net — cross-platform HTTP",
    category = Reference,
    description = "Async HTTP, WebSocket and Server-Sent Events SDK. One public surface, four per-target HTTP transports (reqwest / fetch / NSURLSession / HttpURLConnection).",
    related = ["server-functions", "async-reactivity"],
    concepts = [Net, NetClient, BodyCodec, NetCancelToken],

    section(heading = "Overview") {
        p(code("net"),
          " is the framework's networking SDK: an HTTP ", code("Client"),
          ", a ", code("WebSocket"), ", and a receive-only ",
          code("EventSource"), " for Server-Sent Events. HTTP has one API \
           surface and four transports underneath:"),
        list(
            ["Linux / Windows / terminal: ", code("reqwest"),
             " + rustls, on an I/O runtime thread ", code("net"), " owns"],
            ["Web (wasm32): the browser's ", code("fetch"), " via the framework's own \
              web-glue bindings"],
            ["iOS / macOS / tvOS: ", code("NSURLSession"), " via ",
             code("objc2"), " + ", code("block2")],
            ["Android: ", code("HttpURLConnection"),
             " via JNI on a worker thread"],
        ),
        p("Each backend uses the platform's NATIVE HTTP stack. That's a deliberate \
           choice: apps inherit App Transport Security on iOS, system proxy on \
           Android, certificate-pinning hooks where the OS supplies them, and \
           fetch-cache semantics on web. No 2MB+ rustls/reqwest cohort gets dragged \
           into a 100KB wasm bundle."),
        p("The crate is independently usable — ", code("net"), " doesn't depend \
           on ", code("runtime-core"),
          " or the reactive system. Its only framework dependency is ",
          code("web-glue"), " (the browser bindings), and only on wasm. Drop \
           it into a plain Rust binary."),
    },

    section(heading = "The shape") {
        code(rust, r##"
            use net::{Client, Json};

            let client = Client::new();

            let user: User = client
                .get("https://api.example.com/users/1")
                .header("Authorization", "Bearer xyz")
                .send()
                .await?
                .json()
                .await?;

            let created: User = client
                .post("https://api.example.com/users")
                .body(Json(&CreateUser { name: "Alice".into() }))
                .send()
                .await?
                .json()
                .await?;
        "##),
        p("The HTTP types:"),
        list(
            [code("Client"), " — cheap-to-clone (", code("Arc"),
             " inside), holds a base URL, default headers, default timeout, and a \
              per-process connection pool."],
            [code("ClientBuilder"), " — ",
             code("Client::builder().base_url(...).default_header(...).build()"), "."],
            [code("RequestBuilder"),
             " — fluent. Headers, query params, body, timeout, cancellation. \
              Returned by ", code("client.get(url)"), " / ", code("post(url)"),
             " / etc."],
            [code("Response"),
             " — the awaited result. Status, headers, body (buffered into \
              memory)."],
            [code("Method"), " — closed enum: ", code("Get"), ", ", code("Post"),
             ", ", code("Put"), ", ", code("Patch"), ", ", code("Delete"),
             ", ", code("Head"), ", ", code("Options"), "."],
            [code("Headers"),
             " — order-preserving case-insensitive multi-map. ",
             code("Set-Cookie"), " repeats are kept."],
        ),
    },

    section(heading = "Pluggable bodies") {
        p("Body semantics go through two traits:"),
        code(rust, r##"
            pub trait IntoBody {
                fn into_body(self) -> Result<(Vec<u8>, Option<&'static str>), Error>;
            }

            pub trait FromBody: Sized {
                fn from_body(bytes: Vec<u8>, content_type: Option<&str>) -> Result<Self, Error>;
            }
        "##),
        p("Built-in impls: ", code("()"), ", ", code("Vec<u8>"),
          ", ", code("String"), ", ", code("&'static str"),
          ", plus the feature-gated ", code("Json<T>"), " and ", code("Form<T>"),
          " wrappers."),
        code(rust, r##"
            client.post(url).body(Json(&payload)).send().await?;
            client.post(url).body(Form(&form_data)).send().await?;
            client.post(url).body(b"raw bytes".to_vec()).send().await?;
            client.post(url).body(()).send().await?;   // no body, no content-type
        "##),
        p(code("Json<T>"), " and ", code("Form<T>"),
          " set their content-type defaults — overridden if you already added a ",
          code(".header(\"Content-Type\", ...)"), "."),
        p("The body builders return the builder, not a ", code("Result"),
          ". A body that fails to encode is held on the request and returned \
           as ", code("Error::Serialize"), " from ", code(".send()"),
          ", so one ", code("?"), " after ", code(".await"), " covers it."),
    },

    section(heading = "Downstream wire formats") {
        p("The traits are the extensibility point. Server functions (the SDK on \
           top of ", code("net"), ") send their arguments as JSON with ",
          code("net::Json"), "."),
        p("If you want msgpack, CBOR, protobuf, postcard, etc., write a small \
           wrapper ", code("Foo<T>(pub T)"),
          " in your own crate and implement both traits. No changes to ",
          code("net"), " required."),
    },

    section(heading = "Convenience methods") {
        p("Response exposes the common decode paths without going through ",
          code("FromBody"), ":"),
        code(rust, r##"
            let resp = client.get(url).send().await?;
            let body_bytes: Vec<u8> = resp.bytes().await?;
            let body_text: String   = resp.text().await?;
            let body_json: User     = resp.json().await?;
        "##),
        p("For request bodies, ", code(".json(&value)"), " / ", code(".form(&value)"),
          " are sugar that takes ", code("&T"), " so the caller doesn't move the \
           value:"),
        code(rust, r##"
            client.post(url).json(&payload).send().await?;   // = .body(Json(&payload))
        "##),
    },

    section(heading = "Cancellation") {
        p(code("net"),
          " ships its own cancellation primitive so it can be used without pulling \
           in the framework's reactive system."),
        code(rust, r##"
            let (handle, token) = net::cancel_token();

            let request = client.get(url).cancel_on(token).send();

            // later, anywhere:
            handle.cancel();
        "##),
        p("When ", code("cancel()"), " fires:"),
        list(
            ["If the request is already in flight, the underlying transport aborts \
              it (reqwest drops the future, the browser calls ",
             code("AbortController::abort"), ", iOS sends the task ",
             code("cancel"), ", Android attaches a fresh JNI thread and calls ",
             code("HttpURLConnection.disconnect"), ")."],
            ["If the request hasn't started yet, it short-circuits before hitting \
              the network."],
            ["The future returned by ", code(".send()"),
             " resolves with ", code("Err(Error::Cancelled)"), "."],
        ),
        p("The ", code("(handle, token)"),
          " pair is shaped that way so a single handle can fire many requests — \
           clone the token, attach it to N in-flight requests, and ",
          code("handle.cancel()"), " aborts the whole fan-out."),
    },

    section(heading = "Bridging to other cancel systems") {
        p(code("net::CancelToken"), " doesn't know about ",
          code("runtime-core::ResourceCancel"), " or anything else. To bridge:"),
        code(rust, r##"
            let (handle, token) = net::cancel_token();
            resource_cancel.on_cancel(move || handle.cancel());
        "##),
        p("Whenever the higher-level system cancels, the net side fires. (Server \
           functions wrap this pattern into ",
          code("server::with_cancel(resource_cancel, future)"),
          " so authors don't write it by hand — see ",
          link("Server functions", to = "server-functions"), ".)"),
    },

    section(heading = "Error type") {
        p("Single enum, transport-agnostic:"),
        code(rust, r##"
            pub enum Error {
                InvalidUrl(String),
                Network(String),
                Timeout,
                Cancelled,
                Status { code: u16, body: Option<String> },
                Serialize(String),
                Deserialize(String),
                Offline,
                Other(String),
            }
        "##),
        p(code("Status"), " is only produced when you opt in via ",
          code("Response::error_for_status()"),
          " — otherwise 4xx and 5xx come back as normal Response values, and your \
           code decides what to do with the status code."),
    },

    section(heading = "Configuration patterns") {
        p("A configured client:"),
        code(rust, r##"
            let api = Client::builder()
                .base_url("https://api.example.com")
                .default_header("Authorization", &format!("Bearer {}", token))
                .timeout(Duration::from_secs(30))
                .build();

            let user: User = api.get("/users/1").send().await?.json().await?;
        "##),
        p("Relative URLs are joined to ", code("base_url"),
          "; absolute URLs always win (useful for the rare cross-origin call)."),
        p("Per-request overrides — anything set on the client can be overridden \
           per-request:"),
        code(rust, r##"
            api.post("/upload")
                .header("Content-Type", "application/octet-stream")   // wins over Json's default
                .timeout(Duration::from_secs(120))                    // wins over client default
                .body(bytes)
                .send().await?;
        "##),
    },

    section(heading = "Per-platform notes") {
        p("For the most part, you don't have to think about which transport is \
           running. A few platform-specific notes for the people who care:"),
        list(
            ["Web: the browser's ", code("fetch"),
             ", called through web-glue bindings. Every request carries an ",
             code("AbortController"), ": a cancel, an expired ", code("timeout"),
             ", or dropping the ", code("send"),
             " future aborts the browser request. Bodies cross as raw bytes, \
              never decoded as text."],
            ["iOS: each request gets a ", code("NSURLSessionDataTask"),
             " against ", code("NSURLSession.sharedSession"),
             ". The completion block bridges back to Rust via a ",
             code("futures-channel"),
             " oneshot. Cancellation sends ", code("[task cancel]"),
             "; the resulting ", code("NSURLErrorCancelled"), " (-999) maps to ",
             code("Error::Cancelled"), "."],
            ["Android: each request spawns a worker thread that attaches to the \
              JavaVM via ", code("ndk_context"),
             ". The connection is promoted to a JNI GlobalRef so a cancel watcher \
              on a separate thread can call ", code("disconnect()"),
             " mid-flight."],
            ["macOS / tvOS: the same ", code("NSURLSession"),
             " transport as iOS."],
            ["Linux / Windows / terminal: reqwest with rustls, run on one \
              process-wide runtime thread ", code("net"),
             " owns, so the host doesn't need to provide a Tokio runtime. \
              Dropping the future aborts the exchange; a ", code("poll_fn"),
             " race keeps the cancel-token contract uniform across all \
              transports."],
        ),
    },

    section(heading = "WebSocket and Server-Sent Events") {
        p(code("net::WebSocket"), " is the bidirectional sibling of ",
          code("Client"), ": ", code("connect(url)"), ", ", code("send"),
          ", ", code("recv"), ", ", code("close"),
          ", and it closes on drop. Messages are ", code("WsMessage::Text"),
          " or ", code("WsMessage::Binary"), "; ", code("close_status()"),
          " reports the close code and reason the same way on every target."),
        code(rust, r##"
            use net::{WebSocket, WsMessage};

            let mut ws = WebSocket::connect("wss://echo.example.com").await?;
            ws.send(WsMessage::Text("hello".into()))?;
            while let Some(msg) = ws.recv().await {
                match msg? {
                    WsMessage::Text(t) => println!("{t}"),
                    WsMessage::Binary(b) => println!("{} bytes", b.len()),
                }
            }
        "##),
        p(code("net::EventSource"), " reads a ", code("text/event-stream"),
          " (what a ", code("#[sse]"), " server endpoint serves): ",
          code("connect(url)"), ", then ", code("recv()"),
          " yields each event's ", code("data:"), " payload."),
        p("The web arm uses the browser's ", code("WebSocket"), " and ",
          code("EventSource"), ". Native targets run ", code("tungstenite"),
          " on an I/O thread for WebSocket; Android supports ",
          code("ws://"), " only (no bundled TLS)."),
    },

    section(heading = "What's not included") {
        list(
            ["Streaming HTTP response bodies. ", code("Response"),
             " bodies are buffered in memory; use ", code("EventSource"),
             " or ", code("WebSocket"), " for data that arrives over time."],
            ["Multipart uploads."],
            ["Cookies. Each platform's native HTTP stack manages its own cookie \
              jar; there is no cross-platform cookie API."],
        ),
    },

    section(heading = "Where to read more") {
        list(
            [link("Server functions", to = "server-functions"),
             " — the canonical consumer of ", code("net"),
             ". Every ", code("#[server]"),
             " call site routes through this crate on the client side."],
            [link("Async reactivity", to = "async-reactivity"),
             " — the higher-level reactive primitives that consume ",
             code("net"), "'s futures."],
        ),
    },
}
