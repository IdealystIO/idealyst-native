# Serverless (AWS Lambda) — `idealyst build --serverless-lambda`

`idealyst build --serverless-lambda` packages an app's `#[server]` functions as
an AWS Lambda. It builds on the [`server-aws`](../crates/api/server-aws) adapter:
`server::router()` is an `axum::Router`, an `axum::Router` is a `tower::Service`,
and the Lambda Rust runtime (`lambda_http`) runs any such service — so the whole
server-fn API becomes one Lambda, fronted by a Function URL or API Gateway.

```
idealyst build --serverless-lambda            # arm64, debug
idealyst build --serverless-lambda --release  # arm64, release (deploy build)
idealyst build --serverless-lambda --arch x86_64 --release
```

## What it does

The app crate stays platform-agnostic — the same `pub fn app()` and `#[server]`
fns you already have. The command generates an ephemeral wrapper crate under
`target/idealyst/<project>/serverless-lambda/wrapper/` whose `main` runs
`server_aws::run()`, then builds it with [`cargo lambda`](https://cargo-lambda.info)
into a `bootstrap` for the `provided.al2023` custom runtime.

It stages a self-contained deploy/test context at `dist/serverless-lambda/`:

```
dist/serverless-lambda/
  bootstrap        # the Lambda handler binary
  Dockerfile       # FROM public.ecr.aws/lambda/provided:al2023 (bundles the RIE)
```

### Requirements

- `cargo lambda` — `cargo install cargo-lambda` (or `brew install cargo-lambda`).
- Cross-compiling from macOS also needs Zig (`brew install zig`), which
  cargo-lambda uses as its linker.

### The force-link line

`#[server]` fns register through `inventory::submit!`. A binary must *reference
something* from the crate holding those registrations, or the linker drops them
and `server::router()` registers zero routes (every `/_srv/<fn>` then 404s). The
generated wrapper references `<lib>::app` for exactly this reason; the runtime
also prints a startup warning when it finds no routes, as a backstop.

## Local testing — the real image, via the RIE

The AWS `provided.al2023` base image bundles the **Lambda Runtime Interface
Emulator (RIE)**, so the exact image you deploy also runs locally — no
AWS-service emulator (LocalStack) required:

```
cd dist/serverless-lambda
docker build  --platform linux/arm64 -t my-lambda .
docker run --rm --platform linux/arm64 -p 9000:8080 my-lambda

# Invoke it the way Lambda does — POST a Function-URL v2 event envelope:
curl -s "http://localhost:9000/2015-03-31/functions/function/invocations" \
  -d '{"version":"2.0","rawPath":"/_srv/submit_contact",
       "requestContext":{"http":{"method":"POST","path":"/_srv/submit_contact"}},
       "headers":{"content-type":"application/json"},
       "body":"{\"input\":{...}}","isBase64Encoded":false}'
```

For a Rust-native inner-loop without Docker, `cargo lambda watch` /
`cargo lambda invoke` run the function as a local HTTP server; the Docker + RIE
path above is what exercises the actual deployable image.

## Deploy

The staged `bootstrap` is a standard cargo-lambda artifact:

```
cargo lambda deploy my-lambda --enable-function-url \
  --env-var CONTACT_TABLE=... --env-var CONTACT_FROM=...
```

or ship the Docker image to ECR and create an image-based function. The
[contact-form-lambda example](../crates/api/server-aws/examples/contact-form-lambda)
carries a SAM `template.yaml` for the CloudFormation path.

## Scope

Unary HTTP `#[server]` fns (including the `/_srv/_batch` route) port as-is.
`#[channel]`/`#[subscription]` (WebSockets) and `#[sse]` need separate adapters
(an API Gateway WebSocket API; Function-URL response streaming) and are not part
of this target. See [server-functions.md](server-functions.md) and the
`server-aws` crate docs.

## Other serverless platforms

`serverless-lambda` is the AWS target. Each serverless platform has its own
runtime contract, so each is a sibling target (`serverless-<platform>`) built on
its own adapter crate, added as the need arises — the wrapper-generation pattern
here is the template for them.
