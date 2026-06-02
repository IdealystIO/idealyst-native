//! Parsing for the `i18n! { … }` DSL.
//!
//! ```text
//! locales: { En = "en" (default), Fr = "fr", Ja = "ja" (lazy) }
//!
//! greeting(name) { En: "Hello, {name}", Fr: "Bonjour, {name}" }
//! hello          { En: "Hi",            Fr: "Salut" }
//! ```

use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{braced, parenthesized, token, Ident, LitStr, Token};

/// One locale in the `locales: { … }` header.
pub struct LocaleDecl {
    /// PascalCase enum variant (e.g. `En`).
    pub variant: Ident,
    /// BCP-47-ish code string literal (e.g. `"en"`).
    pub code: LitStr,
    /// `(default)` — fallback + reference locale. Exactly one required.
    pub is_default: bool,
    /// `(lazy)` — opt-in: strings live in a fetched pack, not the binary.
    pub is_lazy: bool,
}

impl Parse for LocaleDecl {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let variant: Ident = input.parse()?;
        input.parse::<Token![=]>()?;
        let code: LitStr = input.parse()?;

        let mut is_default = false;
        let mut is_lazy = false;
        if input.peek(token::Paren) {
            let content;
            parenthesized!(content in input);
            let modifier: Ident = content.parse()?;
            match modifier.to_string().as_str() {
                "default" => is_default = true,
                "lazy" => is_lazy = true,
                other => {
                    return Err(syn::Error::new_spanned(
                        &modifier,
                        format!("unknown locale modifier `{other}`; expected `default` or `lazy`"),
                    ));
                }
            }
            if !content.is_empty() {
                return Err(content.error("expected a single `default` or `lazy` modifier"));
            }
        }

        Ok(LocaleDecl { variant, code, is_default, is_lazy })
    }
}

/// One translation of one message: `Variant: "template"`.
pub struct Entry {
    pub variant: Ident,
    pub template: LitStr,
}

impl Parse for Entry {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let variant: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let template: LitStr = input.parse()?;
        Ok(Entry { variant, template })
    }
}

/// One message: a name, an optional typed argument list, and a brace block
/// of per-locale translations.
pub struct Message {
    pub name: Ident,
    pub args: Vec<Ident>,
    pub entries: Vec<Entry>,
}

impl Parse for Message {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name: Ident = input.parse()?;

        let mut args = Vec::new();
        if input.peek(token::Paren) {
            let content;
            parenthesized!(content in input);
            let parsed: Punctuated<Ident, Token![,]> =
                content.parse_terminated(Ident::parse, Token![,])?;
            args = parsed.into_iter().collect();
        }

        let block;
        braced!(block in input);
        let entries: Punctuated<Entry, Token![,]> =
            block.parse_terminated(Entry::parse, Token![,])?;

        Ok(Message { name, args, entries: entries.into_iter().collect() })
    }
}

/// The whole macro input.
pub struct I18nInput {
    pub locales: Vec<LocaleDecl>,
    pub messages: Vec<Message>,
}

impl Parse for I18nInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let kw: Ident = input.parse()?;
        if kw != "locales" {
            return Err(syn::Error::new_spanned(
                &kw,
                "expected the `locales: { … }` header first",
            ));
        }
        input.parse::<Token![:]>()?;

        let block;
        braced!(block in input);
        let decls: Punctuated<LocaleDecl, Token![,]> =
            block.parse_terminated(LocaleDecl::parse, Token![,])?;
        let locales = decls.into_iter().collect();

        // Optional separator between the header and the first message.
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }

        let mut messages = Vec::new();
        while !input.is_empty() {
            messages.push(input.parse::<Message>()?);
        }

        Ok(I18nInput { locales, messages })
    }
}

/// Extract `{name}` placeholder names from a template, honoring `{{`/`}}`
/// escapes. Shared by validation (compile-time checking) — mirrors the
/// runtime substitutor's scanning so the two agree on what a placeholder is.
pub fn placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    continue;
                }
                let mut name = String::new();
                let mut closed = false;
                for nc in chars.by_ref() {
                    if nc == '}' {
                        closed = true;
                        break;
                    }
                    name.push(nc);
                }
                if closed {
                    let trimmed = name.trim();
                    if !trimmed.is_empty() {
                        out.push(trimmed.to_string());
                    }
                }
            }
            '}' => {
                if chars.peek() == Some(&'}') {
                    chars.next();
                }
            }
            _ => {}
        }
    }
    out
}
