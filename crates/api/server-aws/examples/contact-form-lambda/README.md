# contact-form-lambda

A serverless contact form, end to end, on the idealyst `#[server]` SDK:

**submit a form → store in DynamoDB → email a notification (SES)** — running as
a single AWS Lambda, with the *same* function running locally for development.

It's the reference for the **`server-aws`** adapter (`crates/api/server-aws`),
which runs the server-fn router on AWS Lambda with a one-line `main`.

## How it fits together

One `#[server] async fn submit_contact(..)` in [src/lib.rs](src/lib.rs) compiles
two ways:

- **client** (default features) → a typed RPC stub the web form calls.
- **server** (`--features server`) → the real body: validate, `PutItem` into
  DynamoDB, `SendEmail` via SES.

The server side is just the existing `server::router()` (an `axum::Router`,
which is a `tower::Service`) handed to the Lambda runtime by `server-aws`:

```rust
// src/bin/lambda.rs
#[tokio::main]
async fn main() -> Result<(), server_aws::Error> {
    server_aws::run().await   // = lambda_http::run(server::router())
}
```

Every `#[server]` fn in the crate is served — `submit_contact` here, plus the
`/_srv/_batch` route — at `POST /_srv/<fn>`, exactly as the local server serves
them. Nothing in the function bodies is Lambda-specific.

> **Scope.** Unary HTTP `#[server]` fns port as-is. The streaming siblings
> (`#[channel]`/`#[subscription]` over WebSockets, and `#[sse]`) do **not** map
> onto plain Lambda request/response — they need an API Gateway WebSocket API
> and Function-URL response-streaming respectively. See the `server-aws` crate
> docs.

## Run it locally (no AWS needed)

`CONTACT_DRY_RUN=1` skips the AWS calls and logs instead, so you can exercise
the whole round-trip with no credentials or provisioned resources:

```sh
CONTACT_DRY_RUN=1 cargo run -p contact-form-lambda --bin local --features server
```

Then POST a submission. Server-fn wire args are a **tuple**, so a single
struct arg is wrapped in a one-element array:

```sh
curl -s localhost:3000/_srv/submit_contact \
  -H 'content-type: application/json' \
  -d '[{"name":"Ada","email":"ada@example.com","message":"hello"}]'
# → {"Ok":"<uuid>"}     and the server logs: [dry-run] would store + notify: ...
```

Drop `CONTACT_DRY_RUN` (and set `CONTACT_TABLE` / `CONTACT_FROM` / `CONTACT_TO`)
to hit real AWS using your default credential chain.

## Deploy to AWS

Install [cargo-lambda](https://www.cargo-lambda.info/), then either path:

### A. cargo-lambda (quickest)

```sh
cargo lambda build --release --arm64 --features server --bin lambda

cargo lambda deploy contact-form \
  --env-var CONTACT_TABLE=ContactSubmissions \
  --env-var CONTACT_FROM=noreply@yourdomain.com \
  --env-var CONTACT_TO=you@yourdomain.com \
  --enable-function-url
```

You provision the table + SES identity yourself:

```sh
aws dynamodb create-table --table-name ContactSubmissions \
  --attribute-definitions AttributeName=id,AttributeType=S \
  --key-schema AttributeName=id,KeyType=HASH \
  --billing-mode PAY_PER_REQUEST
aws ses verify-email-identity --email-address noreply@yourdomain.com
```

The function's execution role needs `dynamodb:PutItem` on the table and
`ses:SendEmail`.

### B. SAM (infrastructure-as-code)

[template.yaml](template.yaml) declares the function, the table, the IAM policy,
and a CORS-enabled Function URL together — the shape an "export to serverless"
tool would emit from the server-fn inventory:

```sh
sam build --beta-features        # shells out to cargo-lambda
sam deploy --guided \
  --parameter-overrides NotifyFrom=noreply@yourdomain.com NotifyTo=you@yourdomain.com
```

The stack output `ApiBase` is the Function URL.

## Point the web client at the deployed API

A static web build and the Lambda live on different origins, so bake the
Function URL in at build time — `configure_server()` reads `CONTACT_API_BASE`,
falling back to same-origin (which is what the local server serves):

```sh
CONTACT_API_BASE=https://<id>.lambda-url.<region>.on.aws \
  idealyst build --web examples/contact-form-lambda
```

## Environment variables (server build)

| Var                | Required | Meaning                                    |
| ------------------ | -------- | ------------------------------------------ |
| `CONTACT_TABLE`    | yes¹     | DynamoDB table name                        |
| `CONTACT_FROM`     | yes¹     | Verified SES sender address                |
| `CONTACT_TO`       | yes¹     | Notification recipient                     |
| `CONTACT_DRY_RUN`  | no       | `1`/`true` → skip AWS calls, log instead   |
| `PORT`             | no       | local bin only; default 3000               |

¹ Not required when `CONTACT_DRY_RUN` is set.
