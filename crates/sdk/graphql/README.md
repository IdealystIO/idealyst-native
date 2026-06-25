# `graphql` — typed GraphQL client + server execution

Full GraphQL for the framework, built on the established Rust ecosystem:

- **Client typing** — [`graphql_client`] codegen. Author `.graphql` operation
  files against a schema and derive typed `Variables` / `ResponseData` with
  `#[derive(GraphQLQuery)]`.
- **Server** — [`async-graphql`] (behind the `server` feature) for schema
  definition, validation, execution, introspection, and SDL export.

## Works *with* server functions, doesn't *depend* on them

The client is written against one trait, `Transport`. The crate has **no
dependency on the `server` (server-functions) SDK** — the same decoupling the
`sync` SDK uses. Two ways to wire it:

1. **On the server-functions stack** — write one `#[server]` fn and bridge it
   with `graphql_transport!`. GraphQL then rides the server-functions HTTP
   path and reuses its auth, CSRF, credentials, and per-platform config:

   ```rust
   // shared (both builds)
   graphql::graphql_transport!(AppGraphql, graphql_endpoint);
   let client = graphql::GraphqlClient::new(std::rc::Rc::new(AppGraphql));

   // server build only
   #[server]
   pub async fn graphql_endpoint(req: graphql::GraphqlRequest)
       -> Result<graphql::GraphqlResponse, ServerError> {
       let schema = server::use_state::<Arc<MySchema>>()
           .ok_or_else(|| ServerError::failed("schema not installed"))?;
       Ok(graphql::execute_request(schema.as_ref(), req).await)
   }
   ```

2. **Off it** — point `HttpTransport` at any GraphQL endpoint, and on the
   server run `graphql::execute_request` from any handler:

   ```rust
   let client = graphql::GraphqlClient::http("https://api.example.com/graphql");
   ```

## Reactive use

`use_query` and `use_mutation` bind operations to the reactive runtime — they
wrap `runtime_core`'s `resource` / `mutation`, so a component re-renders as
results arrive and you get `data()` / `error()` / `loading()` / `refetch()`
(queries) and `trigger()` / `state()` (mutations) for free.

The generated `ResponseData` must be `Clone` for these (resource/mutation
value types are `Clone`): add `response_derives = "Debug, Clone"` to the
`#[derive(GraphQLQuery)]` attribute.

## Defining the schema (server)

Define the schema with `async-graphql` as usual; export its SDL with
`graphql::sdl(&schema)` to feed `graphql_client`'s `schema_path`, so client
and server agree on types by construction.

> async-graphql's `#[derive(SimpleObject)]` / `#[Object]` macros expand to
> `::async_graphql::…` paths, so a crate that *defines* a schema must depend
> on `async-graphql` directly (the SDK re-exports it for non-derive use, but
> the derives need it at the crate root). Pin the same major the SDK uses so
> cargo unifies to one crate and the `Schema` types match.

## Scope (v1)

Queries + mutations, end to end. Subscriptions are a documented extension
seam — async-graphql already supports `execute_stream`; a
`SubscriptionTransport` over a WebSocket (the `net` SDK, or a
server-functions `#[channel]`) layers on without reworking the above.

See [`examples/graphql-demo`](../../../examples/graphql-demo) for a full-stack
app.

[`graphql_client`]: https://crates.io/crates/graphql_client
[`async-graphql`]: https://crates.io/crates/async-graphql
