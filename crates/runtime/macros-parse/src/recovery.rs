//! The IDE-recovery shell and token salvage.
//!
//! A `ui!` body that fails to parse must not black out the editor. This
//! module re-surfaces as much of the raw input as still parses, in
//! dead-but-type-checked positions, so rust-analyzer keeps completion,
//! hover and go-to-def for the parts of the block that ARE well-formed.
//!
//! It lives with the parser because it is the parser's failure path: it
//! consumes raw tokens and knows the same grammar, and neither half is
//! useful without the other.

use proc_macro2::{Delimiter, Spacing, TokenStream as TokenStream2, TokenTree};
use quote::{quote, ToTokens};
use syn::Expr;

/// Wrap an emission tail (the real build chain on the happy path, or the
/// diagnostic + empty view on the recovery path) in the SHARED shell that
/// carries the salvage closure.
///
/// Why the happy path carries a salvage copy at all: rust-analyzer's
/// completion inside a macro call expands the buffer TWICE — once as
/// written ("real") and once with a placeholder ident spliced at the
/// cursor ("speculative") — and then resolves nodes of the speculative
/// expansion against the real expansion's HIR **by text range**. That
/// only works when the two expansions are textually aligned up to the
/// cursor. A mid-typing state like a dangling `c.` above an existing
/// statement token-glues into VALID code (`c.` + `c.reset()` ⇒
/// `c.c.reset()`), so the real buffer takes the happy path while the
/// speculative one hits recovery — two structurally unrelated expansions,
/// and the receiver's range lands on an arbitrary expression of the real
/// one (observed: dot-completion on a `CounterHandle` offering `Ref`
/// methods, because the offset happened to cover `counter`). Emitting the
/// SAME salvage closure, at the SAME offset, in BOTH paths restores the
/// alignment for every (valid, mid-typing) buffer pair. Span line info
/// can't help instead: rust-analyzer's proc-macro server reports 1:0 for
/// every span (verified empirically), so the glue is undetectable at
/// macro level.
///
/// The shell is emitted ONLY under an IDE-style expansion host (detected
/// by [`host_has_no_span_lines`]): under real rustc the happy expansion
/// is byte-identical to the pre-shell output. That keeps real builds
/// clean three ways — no `unexpected_cfgs`-style lint games in consumer
/// crates, no type-checking of the salvage copy (a `move` closure in the
/// salvage would steal captures from the real chain and break VALID code
/// with E0382), and no salvage noise in `cargo expand` or in the error
/// output of genuinely broken builds. Flycheck runs rustc, so editor
/// diagnostics never see the salvage either; rust-analyzer's own
/// diagnostics don't include borrowck.
///
/// The diagnostic (recovery path) must come AFTER the closure: parse
/// error messages differ between the real and speculative buffers, and a
/// leading `compile_error!("…")` of a different length would shift every
/// salvage offset — breaking the very alignment this shell exists for.
pub fn emit_shell(input: &TokenStream2, rest: TokenStream2) -> TokenStream2 {
    if !host_has_no_span_lines(input) {
        return quote! { ::runtime_core::IntoElement::into_element(#rest) };
    }
    let salvaged = salvage_stmts(input.clone());
    quote! {
        ::runtime_core::IntoElement::into_element({
            #[allow(unused, unreachable_code, clippy::all)]
            let __ui_recover = || {
                #( #salvaged )*
            };
            #rest
        })
    }
}

/// True when the expansion host provides no real span positions — the
/// discriminator between rustc and IDE proc-macro servers. rust-analyzer's
/// server reports a degenerate zero-width `1:0` (or `0:0`) for EVERY
/// token span (verified empirically — which is also why a line-aware
/// parse can't detect the `c.`-glues-with-next-line shape and this shell
/// exists at all); under rustc ≥1.88 with proc-macro2's `span-locations`,
/// real tokens carry real positions, and a multi-token input can never be
/// all zero-width at column 0. proc-macro2's fallback spans (unit tests,
/// non-rustc hosts) are degenerate the same way, so tests exercise the
/// shell path. If rust-analyzer ever starts reporting real span lines,
/// this returns false there and completion inside broken `ui!` bodies
/// regresses to the bare-`compile_error!` baseline — revisit the gate
/// (a `#[cfg(rust_analyzer)]` emission, which currently trips
/// `unexpected_cfgs` in consumer crates, becomes the alternative).
fn host_has_no_span_lines(input: &TokenStream2) -> bool {
    input.clone().into_iter().all(|tt| {
        let (s, e) = (tt.span().start(), tt.span().end());
        s == e && s.line <= 1 && s.column == 0
    })
}

/// Emit a *recovery* expansion for a `ui!`/`jsx!` body that failed to
/// parse. Two jobs:
///
/// 1. Re-emit the real `compile_error!` (with the parser's span) so the
///    build still fails with the correct diagnostic at the correct place.
/// 2. Re-surface as much of the raw input as possible in
///    dead-but-type-checked positions, so rust-analyzer keeps full type
///    info (completion, hover, go-to-def) for the parts of the block that
///    *are* well-formed — i.e. everything except the token you're mid-way
///    through typing. Without this, a single in-progress expression turns
///    the entire `ui! { … }` into an opaque `compile_error!` and the IDE
///    goes dark for the whole block.
///
/// Three salvage forms, matched to what completion needs per position:
///
/// - **Prop values** (`label = <expr>`): the RHS as a value expression —
///   variable/method completion inside prop values.
/// - **Component invocations** (`PascalTag(…)`): a STRUCT LITERAL over
///   the tag (which aliases the props type) with every leading ident of
///   the prop list as a `field: todo!()` entry — this is what makes
///   rust-analyzer complete prop NAMES at `Counter(sta|)`, the single
///   most common mid-typing state. See [`salvage_component_invocations`].
/// - **Statement runs** (children blocks): iterated longest-valid-prefix
///   expression salvage, so ONE broken child doesn't eat its siblings —
///   bare exprs, half-typed tag names (value-position ident completion),
///   primitive calls, and the conditions of `if`/`for`/`match` headers
///   whose ui!-flavored bodies aren't valid Rust statements. See
///   [`salvage_statement_run`].
///
/// The salvaged expressions live inside a never-called closure: they're
/// analyzed but never executed, and `&(expr)` avoids moving out of
/// the user's bindings. The whole thing still evaluates to an `Element`
/// so the surrounding code type-checks as far as it can. The closure is
/// emitted through [`emit_shell`] — shared with the HAPPY path — so the
/// real and speculative expansions rust-analyzer compares stay textually
/// aligned regardless of which path each buffer takes (see `emit_shell`
/// for why that alignment is what makes completion resolve at all).
///
/// Everything emitted here is guaranteed-valid Rust syntax: salvage only
/// keeps token runs that successfully parse as a `syn::Expr` (plus the
/// synthesized struct literals, valid by construction). If it emitted
/// unparseable tokens, rust-analyzer's own expansion of `ui!` would fail
/// and we'd be worse off than the `compile_error!` baseline. Original
/// token SPANS are preserved throughout (tokens are reused, never
/// re-created) — that's what lets RA map completions back to the cursor.
pub fn emit_recovery(input: TokenStream2, err: &syn::Error) -> TokenStream2 {
    let diag = err.to_compile_error();
    emit_shell(
        &input,
        quote! {
            #diag
            ::runtime_core::view(::std::vec::Vec::new())
        },
    )
}

/// Walk a raw token stream and collect salvage STATEMENTS (most are
/// `let _ = &(expr);` wrappers; `let` statements are preserved verbatim),
/// preserving spans. Used by [`emit_shell`] for BOTH the happy and the
/// recovery expansion.
///
/// Strategy: within each token group, split on top-level commas. A
/// segment shaped `ident = <tokens>` (a prop assignment — the lone `=`
/// is `Spacing::Alone`, which rules out `==`/`=>`/`<=`) yields its RHS as
/// a candidate expression. We also recurse into every nested group so
/// children blocks and call arguments get salvaged too. We deliberately
/// do NOT try to parse whole segments as expressions: `Card { … }` is a
/// syntactically valid struct literal but semantically bogus (Card isn't
/// a struct), and emitting it would inject spurious type errors that
/// drown out the real completions.
///
/// PREFIX-STABILITY INVARIANT: because the happy and recovery paths both
/// carry this salvage, and rust-analyzer resolves its speculative
/// expansion against the real one BY TEXT RANGE, the salvage of two
/// inputs that agree up to a position must agree in output up to that
/// position. Every decomposition decision below is therefore made from
/// the FRONT of the token run — never by whether some enclosing run
/// happens to parse as a whole (which would let tokens after the cursor
/// change output before it). That's why control-flow headers and bare
/// calls decompose canonically even when the full expression is valid.
fn salvage_stmts(stream: TokenStream2) -> Vec<TokenStream2> {
    let mut out = Vec::new();
    salvage_from_stream(stream, &mut out, SalvageCtx::Children);
    out
}

/// Push an expression-shaped salvage as a `let _ = &(expr);` statement.
/// `&(expr)` avoids moving out of the user's bindings.
fn push_expr(out: &mut Vec<TokenStream2>, e: TokenStream2) {
    out.push(quote! { let _ = &(#e); });
}

/// Which grammatical position a token run being salvaged sits in. The
/// `ui!` grammar gives `Pascal ( … )` two meanings by position: in CHILD
/// position it is a component tag (struct-literal salvage → prop-NAME
/// completion), but inside a prop value or handler body it is ordinary
/// Rust — a tuple-struct/enum-variant CONSTRUCTOR call (`P2(w, h)`,
/// `Dir::North(steps)`), and struct-literal-salvaging those plants
/// `E0560 no such field` squiggles from rust-analyzer's pull diagnostics
/// on VALID code (user-hit via the always-on happy shell). So the
/// struct-literal salvage fires only in `Children` context.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SalvageCtx {
    /// Direct children of a tag body — `Pascal(…)` is a component tag.
    Children,
    /// Prop values, call arguments, handler bodies — plain Rust.
    Value,
}

fn salvage_from_stream(stream: TokenStream2, out: &mut Vec<TokenStream2>, ctx: SalvageCtx) {
    let mut segment: Vec<TokenTree> = Vec::new();
    let mut segments: Vec<Vec<TokenTree>> = Vec::new();
    for tt in stream {
        match &tt {
            TokenTree::Punct(p) if p.as_char() == ',' && p.spacing() == Spacing::Alone => {
                segments.push(std::mem::take(&mut segment));
            }
            _ => segment.push(tt),
        }
    }
    if !segment.is_empty() {
        segments.push(segment);
    }

    for seg in segments {
        // Component invocations (`PascalTag ( … )`) get a STRUCT-LITERAL
        // salvage so rust-analyzer offers prop-NAME completion at a
        // half-typed prop — see [`salvage_component_invocations`]. Only
        // in CHILD position, where the grammar says a Pascal call IS a
        // tag (see [`SalvageCtx`]).
        if ctx == SalvageCtx::Children {
            salvage_component_invocations(&seg, out);
        }
        // A prop assignment: `ident = <rhs>`. Salvage the RHS only — the
        // whole segment would parse as an assignment to an undefined
        // name and inject noise. STRUCTURED salvage first: a closure /
        // control-flow RHS whose brace body is what's broken keeps its
        // BINDINGS via scaffolding — `on_click = move || { if let Some(c)
        // = … { c.│ } }` must keep `c` bound, or dot-completion at the
        // cursor degrades to unknown-receiver noise (the bug that made
        // handle methods invisible until a prefix was typed).
        if let Some(rhs) = prop_value_tokens(&seg) {
            if let Some(composite) = try_scaffold(&rhs, SalvageCtx::Value) {
                push_expr(out, composite);
            } else if let Some(expr_ts) = parse_expr_prefix(rhs.clone()) {
                push_expr(out, expr_ts);
            } else {
                // Nothing parseable at this level — groups inside may
                // still hold salvageable content.
                for tt in &rhs {
                    if let TokenTree::Group(g) = tt {
                        salvage_from_stream(g.stream(), out, SalvageCtx::Value);
                    }
                }
            }
            continue;
        }
        // Otherwise treat the segment as a STATEMENT RUN — ui! children
        // are whitespace-separated, so one broken child must not eat its
        // siblings. Owns ALL group recursion for its tokens: a scaffolded
        // body must not ALSO be salvaged flat, or the bound copies get
        // shadowed by unbound duplicates full of unresolved-name noise.
        salvage_statement_run(seg, out, ctx);
    }
}

/// Try to rebuild `HEAD { BODY }` where the BODY is what failed to
/// parse: keep HEAD as REAL scaffolding — preserving pattern bindings
/// (`if let Some(c) = …`, `for x in …`) and closure parameters
/// (`move |e| …`) — around a recursively salvaged body. The candidate is
/// validated by parsing the actual composite, so a header whose block
/// isn't statement-shaped (`match` needs arms, a bogus head, …) returns
/// `None` and falls back to coarser salvage.
fn try_scaffold(toks: &[TokenTree], ctx: SalvageCtx) -> Option<TokenStream2> {
    let TokenTree::Group(g) = toks.last()? else {
        return None;
    };
    if g.delimiter() != Delimiter::Brace || toks.len() < 2 {
        return None;
    }
    let head: TokenStream2 = toks[..toks.len() - 1].iter().cloned().collect();
    let mut inner: Vec<TokenStream2> = Vec::new();
    salvage_from_stream(g.stream(), &mut inner, ctx);
    let candidate = quote! {
        #head {
            #( #inner )*
        }
    };
    syn::parse2::<Expr>(candidate.clone()).ok().map(|_| candidate)
}

/// Scaffold a plain `if`/`for` through the SAME type dispatch the real
/// `ui!` lowering uses, so the salvage accepts exactly what the DSL
/// accepts (see the call site in [`salvage_statement_run`] for the full
/// rationale — the plain-Rust form E0308-squiggles valid reactive
/// conditions under rust-analyzer). Returns the number of consumed
/// tokens on success; `None` (nothing emitted) when the construct is too
/// broken to scaffold this way.
///
/// For `if`, the whole `else if …`/`else` chain is consumed: the else
/// branch becomes the dispatch's second closure, with a chained
/// `else if` salvaged recursively inside it.
fn salvage_dispatch_construct(
    kw: &str,
    toks: &[TokenTree],
    out: &mut Vec<TokenStream2>,
    ctx: SalvageCtx,
) -> Option<usize> {
    let body_at = toks.iter().position(
        |t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace),
    )?;
    let TokenTree::Group(body) = &toks[body_at] else {
        return None;
    };

    if kw == "for" {
        let in_pos = toks[..body_at]
            .iter()
            .position(|t| matches!(t, TokenTree::Ident(i) if i == "in"))?;
        if in_pos < 1 || in_pos + 1 >= body_at {
            return None;
        }
        let pat: TokenStream2 = toks[1..in_pos].iter().cloned().collect();
        let iter: TokenStream2 = toks[in_pos + 1..body_at].iter().cloned().collect();
        let mut body_stmts: Vec<TokenStream2> = Vec::new();
        salvage_from_stream(body.stream(), &mut body_stmts, ctx);
        let candidate = quote! {
            {
                #[allow(unused_imports)]
                use ::runtime_core::{StaticForEach as _, ReactiveForEach as _};
                (#iter).__idealyst_for_each(move |#pat| {
                    #( #body_stmts )*
                    ::std::vec::Vec::new()
                })
            }
        };
        return syn::parse2::<Expr>(candidate.clone()).ok().map(|_| {
            push_expr(out, candidate);
            body_at + 1
        });
    }

    // `if` — a nonempty condition is required.
    if body_at < 2 {
        return None;
    }
    let cond: TokenStream2 = toks[1..body_at].iter().cloned().collect();
    let mut then_stmts: Vec<TokenStream2> = Vec::new();
    salvage_from_stream(body.stream(), &mut then_stmts, ctx);

    let mut consumed = body_at + 1;
    let mut else_stmts: Vec<TokenStream2> = Vec::new();
    if matches!(toks.get(consumed), Some(TokenTree::Ident(i)) if i == "else") {
        match (toks.get(consumed + 1), toks.get(consumed + 2)) {
            (Some(TokenTree::Group(g)), _) if g.delimiter() == Delimiter::Brace => {
                salvage_from_stream(g.stream(), &mut else_stmts, ctx);
                consumed += 2;
            }
            (Some(TokenTree::Ident(i)), _) if i == "if" => {
                let end = if_chain_extent(toks, consumed + 1);
                salvage_statement_run(toks[consumed + 1..end].to_vec(), &mut else_stmts, ctx);
                consumed = end;
            }
            _ => {}
        }
    }
    let candidate = quote! {
        {
            #[allow(unused_imports)]
            use ::runtime_core::{StaticCond as _, ReactiveCond as _};
            (#cond).__idealyst_if(
                move || { #( #then_stmts )* ::std::vec::Vec::new() },
                move || { #( #else_stmts )* ::std::vec::Vec::new() },
            )
        }
    };
    syn::parse2::<Expr>(candidate.clone()).ok().map(|_| {
        push_expr(out, candidate);
        consumed
    })
}

/// Index just past an `if … { … } (else if … { … })* (else { … })?`
/// chain whose `if` sits at `start`. Bodies are single brace-group
/// tokens, so the scan can't be confused by nested constructs.
fn if_chain_extent(toks: &[TokenTree], start: usize) -> usize {
    let mut pos = start;
    loop {
        let Some(b) = toks[pos..].iter().position(
            |t| matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace),
        ) else {
            return toks.len();
        };
        pos += b + 1;
        match (toks.get(pos), toks.get(pos + 1)) {
            (Some(TokenTree::Ident(e)), Some(TokenTree::Group(g)))
                if e == "else" && g.delimiter() == Delimiter::Brace =>
            {
                return pos + 2;
            }
            (Some(TokenTree::Ident(e)), Some(TokenTree::Ident(i)))
                if e == "else" && i == "if" =>
            {
                pos += 2;
            }
            _ => return pos,
        }
    }
}

/// Salvage a statement-shaped token run: repeatedly take the longest
/// valid leading expression, emit it, and continue with the remainder;
/// when no prefix parses, drop one leading token and keep going. Two
/// head shapes are consumed WITHOUT expression salvage:
///
/// - `Ident { … }` (tag-with-children) — parses as a struct literal but
///   is semantically bogus from a children block (`Card { … }` would
///   type-check the wrong way); its braces were already recursed.
/// - `PascalTag ( … )` — parses as a fn call to the component fn, whose
///   arity/types won't match the prop list (noise); the struct-literal
///   salvage from [`salvage_component_invocations`] is the useful
///   surface for it.
fn salvage_statement_run(mut toks: Vec<TokenTree>, out: &mut Vec<TokenStream2>, ctx: SalvageCtx) {
    while !toks.is_empty() {
        let head_pair_to_skip = match (toks.first(), toks.get(1)) {
            (Some(TokenTree::Ident(_)), Some(TokenTree::Group(g)))
                if g.delimiter() == Delimiter::Brace =>
            {
                true
            }
            (Some(TokenTree::Ident(i)), Some(TokenTree::Group(g)))
                if g.delimiter() == Delimiter::Parenthesis && is_pascal(i) =>
            {
                true
            }
            _ => false,
        };
        if head_pair_to_skip {
            // The pair itself isn't expression-salvaged, but its group
            // still holds salvageable content: a BRACE group is a tag's
            // children block (Children ctx), a PAREN group is its prop
            // list (Value ctx — Pascal calls inside are constructors).
            if let Some(TokenTree::Group(g)) = toks.get(1) {
                let inner_ctx = if g.delimiter() == Delimiter::Brace {
                    SalvageCtx::Children
                } else {
                    SalvageCtx::Value
                };
                salvage_from_stream(g.stream(), out, inner_ctx);
            }
            toks.drain(..2);
            continue;
        }

        // `let` statements: a whole, valid `let … ;` is preserved
        // VERBATIM so the binding stays live for the statements after it
        // — `let c = counter.get().unwrap(); c.│` needs `c` bound in the
        // salvage copy or dot-completion has no receiver type. Decided
        // entirely from the front (`let` … first top-level `;`), so it's
        // prefix-stable. A broken/mid-typed `let` falls back to salvaging
        // the `=`-RHS prefix (binding lost — acceptable, the buffer is
        // already broken at exactly that statement).
        if matches!(toks.first(), Some(TokenTree::Ident(i)) if i == "let") {
            let semi = toks.iter().position(
                |t| matches!(t, TokenTree::Punct(p) if p.as_char() == ';'),
            );
            if let Some(semi) = semi {
                let stmt: TokenStream2 = toks[..=semi].iter().cloned().collect();
                if syn::parse2::<syn::Block>(quote! { { #stmt } }).is_ok() {
                    out.push(stmt);
                    toks.drain(..=semi);
                    continue;
                }
            }
            let upto = semi.unwrap_or(toks.len());
            if let Some(eq) = toks[..upto].iter().position(|t| {
                matches!(t, TokenTree::Punct(p)
                    if p.as_char() == '=' && p.spacing() == Spacing::Alone)
            }) {
                if let Some(ts) = parse_expr_prefix(toks[eq + 1..upto].to_vec()) {
                    push_expr(out, ts);
                }
            }
            let drain_to = if semi.is_some() { upto + 1 } else { upto };
            toks.drain(..drain_to);
            continue;
        }

        // Control-flow with a statement-shaped body scaffolds CANONICALLY
        // — even when the whole expression is valid. If only the broken
        // variant scaffolded, a buffer whose glued form parses (`c.` +
        // `c.reset()` ⇒ `c.c.reset()`) would emit `if … { c.bump(10); }`
        // whole while its speculative twin emits the scaffold, and the
        // two expansions would diverge BEFORE the cursor (see
        // [`emit_shell`] on why that kills completion). `match` is
        // excluded: its arms aren't statements, so a scaffold body would
        // flat-salvage the arms and strip their pattern bindings on
        // VALID code; it keeps whole-expression salvage below.
        //
        // WHICH scaffold form depends on the construct:
        // - Plain `if`/`for` go through the SAME type dispatch the real
        //   lowering uses (`__idealyst_if` / `__idealyst_for_each`), NOT
        //   a plain Rust `if`/`for` — `ui!` legally accepts a
        //   `ReadSignal<bool>` condition and a `Signal<Vec<_>>` iterable,
        //   which a plain `if`/`for` rejects, and rust-analyzer surfaces
        //   that as an E0308 squiggle ON VALID CODE via its pull-model
        //   diagnostics (user-hit: `if is_high` where
        //   `is_high = memo(…)`). The dispatch accepts exactly what the
        //   DSL accepts, so the salvage typechecks iff the real code
        //   does; it also keeps `for`-loop items TYPED for either world.
        // - `if let`/`while let`/`while` keep the plain-Rust scaffold:
        //   their conditions are plain Rust in valid code (a `let`
        //   pattern must already match its RHS type; `while` has no
        //   reactive form), and the `let` forms need the real construct
        //   to keep their pattern BINDINGS live.
        if let Some(TokenTree::Ident(kw)) = toks.first() {
            let kw = kw.to_string();
            let let_form =
                matches!(toks.get(1), Some(TokenTree::Ident(i)) if i == "let");
            if (kw == "if" || kw == "for") && !let_form {
                if let Some(consumed) = salvage_dispatch_construct(&kw, &toks, out, ctx) {
                    toks.drain(..consumed);
                    continue;
                }
                // Mid-typed condition / no body yet: fall through to the
                // keyword-condition branch below.
            } else if kw == "if" || kw == "while" {
                if let Some(body_at) = toks.iter().position(|t| {
                    matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)
                }) {
                    if let Some(composite) = try_scaffold(&toks[..=body_at], ctx) {
                        push_expr(out, composite);
                        toks.drain(..=body_at);
                        continue;
                    }
                }
            }
        }

        // Bare call in statement position (`ident ( … )`, lowercase, not
        // chained): decompose CANONICALLY into the callee name plus the
        // salvage of its arguments, even when the whole call is valid —
        // whether the interior parses must not change the output emitted
        // before it (prefix stability again). Chained calls
        // (`foo(x).bar()`) keep whole-expression salvage: decomposing
        // would orphan the chain, and the front-token rule stays
        // deterministic by peeking only at the token after the group.
        if let (Some(TokenTree::Ident(head)), Some(TokenTree::Group(g))) =
            (toks.first(), toks.get(1))
        {
            let chained = matches!(toks.get(2), Some(TokenTree::Punct(p))
                if p.as_char() == '.' || p.as_char() == '?');
            let keyword = matches!(
                head.to_string().as_str(),
                "if" | "while" | "for" | "match" | "move" | "return" | "break"
            );
            if g.delimiter() == Delimiter::Parenthesis
                && !is_pascal(head)
                && !keyword
                && !chained
            {
                push_expr(out, head.to_token_stream());
                salvage_from_stream(g.stream(), out, SalvageCtx::Value);
                toks.drain(..2);
                continue;
            }
        }

        // A LEADING brace group is descended, never block-expr-salvaged
        // whole: `{ for x in rows { … } }` parses as a block expression,
        // and swallowing it would re-emit DSL-flavored contents as plain
        // Rust (the exact type-error class the dispatch scaffolds above
        // exist to prevent) — and break prefix stability, since whether
        // the block parses depends on its interior.
        if matches!(toks.first(), Some(TokenTree::Group(g))
            if g.delimiter() == Delimiter::Brace)
        {
            if let TokenTree::Group(g) = &toks[0] {
                salvage_from_stream(g.stream(), out, ctx);
            }
            toks.remove(0);
            continue;
        }

        let mut n = toks.len();
        let mut consumed = 0;
        while n > 0 {
            let ts: TokenStream2 = toks[..n].iter().cloned().collect();
            if let Ok(expr) = syn::parse2::<Expr>(ts) {
                push_expr(out, expr.to_token_stream());
                consumed = n;
                break;
            }
            n -= 1;
        }
        if consumed > 0 {
            toks.drain(..consumed);
            continue;
        }
        // Nothing parseable starts here. STRUCTURED attempt first: if a
        // brace group follows a header, rebuild `HEAD { salvaged-body }`
        // so pattern bindings survive (`if let Some(c) = … { c.│ }`
        // keeps `c` bound — the difference between real dot-completion
        // and unknown-receiver noise at the cursor). Reached for
        // non-keyword headers (statement-position closures etc.) — the
        // control-flow keywords already tried this above.
        if let Some(body_at) = toks.iter().position(|t| {
            matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)
        }) {
            if body_at > 0 {
                if let Some(composite) = try_scaffold(&toks[..=body_at], ctx) {
                    push_expr(out, composite);
                    toks.drain(..=body_at);
                    continue;
                }
            }
        }
        // Scaffolding didn't apply (no header/brace, or a non-statement
        // body like `match` arms). For control-flow headers, the
        // condition is still salvageable Rust — pull it out so
        // `if fro…` keeps completion, then step over the header. The
        // body group is recursed flat before draining.
        if let Some(TokenTree::Ident(kw)) = toks.first() {
            let kw = kw.to_string();
            if kw == "if" || kw == "while" || kw == "match" || kw == "for" {
                let body_at = toks
                    .iter()
                    .position(|t| {
                        matches!(t, TokenTree::Group(g) if g.delimiter() == Delimiter::Brace)
                    })
                    .unwrap_or(toks.len());
                // `for pat in iter` — the expression is what follows
                // `in`; for the others it's everything after the keyword.
                // `if let PAT = expr` — a bare `let` expression is only
                // valid inside `if`/`while`, so salvage the expr after
                // `=` instead of the whole let (emitting `&(let …)` used
                // to produce a bogus-position error squiggle right on
                // the author's pattern).
                let cond_from = if kw == "for" {
                    toks[..body_at]
                        .iter()
                        .position(|t| matches!(t, TokenTree::Ident(i) if i == "in"))
                        .map(|i| i + 1)
                        .unwrap_or(1)
                } else if matches!(toks.get(1), Some(TokenTree::Ident(i)) if i == "let") {
                    toks[..body_at]
                        .iter()
                        .position(|t| {
                            matches!(t, TokenTree::Punct(p)
                                if p.as_char() == '=' && p.spacing() == Spacing::Alone)
                        })
                        .map(|i| i + 1)
                        .unwrap_or(1)
                } else {
                    1
                };
                if cond_from < body_at {
                    if let Some(ts) = parse_expr_prefix(toks[cond_from..body_at].to_vec()) {
                        push_expr(out, ts);
                    }
                }
                if let Some(TokenTree::Group(g)) = toks.get(body_at) {
                    salvage_from_stream(g.stream(), out, ctx);
                }
                let drain_to = (body_at + 1).min(toks.len());
                toks.drain(..drain_to);
                continue;
            }
        }
        // Mid-typed operator, stray punct, or an unrecognized head —
        // step past it, salvaging a group's interior on the way.
        if let TokenTree::Group(g) = &toks[0] {
            salvage_from_stream(g.stream(), out, ctx);
        }
        toks.remove(0);
    }
}

/// True when the ident starts uppercase — the `ui!` component-tag
/// convention (primitives are lowercase-only).
fn is_pascal(i: &proc_macro2::Ident) -> bool {
    i.to_string().chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

/// For every `PascalTag ( … )` pair in the segment, emit a struct-literal
/// salvage:
///
/// ```ignore
/// Counter { sta: ::core::todo!(), start: ::core::todo!(), ..Default::default() }
/// ```
///
/// The tag doubles as the props type (`#[component]` emits `pub type Tag =
/// TagProps`), so with the ORIGINAL ident spans preserved rust-analyzer
/// treats the half-typed `sta` as a field name of the real props struct
/// and completes `start` — the single most common mid-typing state.
/// `todo!()` types as `!` and coerces to every field type, so the
/// complete prop names add zero type-mismatch noise; only the half-typed
/// name shows an unknown-field error, and that sits under the cursor
/// where an error is expected anyway. `..Default::default()` is valid on
/// every props struct (the `BuildElement: Default` contract).
fn salvage_component_invocations(seg: &[TokenTree], out: &mut Vec<TokenStream2>) {
    // PascalCase enum constructors that pattern/expression code uses
    // constantly — `Some(c)` in an `if let` is NOT a component
    // invocation, and emitting `Some { c: todo!() … }` would plant a
    // type-error squiggle on the author's own pattern.
    const NOT_COMPONENTS: &[&str] = &["Some", "Ok", "Err", "None"];
    for pair in seg.windows(2) {
        let (TokenTree::Ident(name), TokenTree::Group(g)) = (&pair[0], &pair[1]) else {
            continue;
        };
        if g.delimiter() != Delimiter::Parenthesis || !is_pascal(name) {
            continue;
        }
        if NOT_COMPONENTS.contains(&name.to_string().as_str()) {
            continue;
        }
        // Leading ident of each top-level comma segment inside the parens
        // — covers `sta`, `start = 3`, and `start =` alike.
        let mut fields: Vec<proc_macro2::Ident> = Vec::new();
        let mut at_start = true;
        for tt in g.stream() {
            match &tt {
                TokenTree::Punct(p) if p.as_char() == ',' && p.spacing() == Spacing::Alone => {
                    at_start = true;
                }
                TokenTree::Ident(i) if at_start => {
                    fields.push(i.clone());
                    at_start = false;
                }
                _ => at_start = false,
            }
        }
        push_expr(
            out,
            quote! {
                #name {
                    #( #fields: ::core::todo!(), )*
                    ..::core::default::Default::default()
                }
            },
        );
    }
}

/// If `seg` begins with `ident =` (a lone `=`, not `==`/`=>`/…), return
/// the right-hand-side tokens. Otherwise `None`.
fn prop_value_tokens(seg: &[TokenTree]) -> Option<Vec<TokenTree>> {
    match (seg.first(), seg.get(1)) {
        (Some(TokenTree::Ident(_)), Some(TokenTree::Punct(p)))
            if p.as_char() == '=' && p.spacing() == Spacing::Alone =>
        {
            Some(seg[2..].to_vec())
        }
        _ => None,
    }
}

/// Parse the longest prefix of `toks` that forms a valid `syn::Expr`,
/// returning it re-tokenized (spans preserved). Trimming the tail lets us
/// recover `foo` from a half-typed `foo.` and `foo.bar` from `foo.bar(`.
fn parse_expr_prefix(mut toks: Vec<TokenTree>) -> Option<TokenStream2> {
    while !toks.is_empty() {
        let ts: TokenStream2 = toks.iter().cloned().collect();
        if let Ok(expr) = syn::parse2::<Expr>(ts) {
            return Some(expr.to_token_stream());
        }
        toks.pop();
    }
    None
}