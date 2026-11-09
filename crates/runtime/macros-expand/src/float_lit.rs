//! Coercing a component prop's value into its field: `(value).into()`,
//! except for an unsuffixed float literal.
//!
//! `field: (16.0).into()` into an `f32` field leaves the literal's type to
//! inference. `f32: From<{float}>` holds only for `f32`, so rustc settles
//! it through `float_literal_f32_fallback` (rust-lang/rust#154024), a
//! future-incompatibility lint that becomes a hard error. Any generic
//! helper (`fn p<T, V: Into<T>>(v: V) -> T`) hits the same fallback: the
//! literal's type is still only bounded by a trait.
//!
//! The macro cannot name the field's type, so the emission picks the
//! suffix from it at type-check time instead, with only `::core` items
//! (no runtime item, so the macro's output stays compatible with every
//! runtime version):
//!
//! - `__pick(|m| …)` takes a `fn(PhantomData<T>) -> T`. The struct
//!   field's type is the call's expected return type, rustc propagates it
//!   to the fn-pointer argument, and the closure is checked with `T`
//!   already concrete. (A closure bound by an `FnOnce` generic would not
//!   receive it: the expectation for a type parameter carries nothing.)
//! - Inside, auto-ref method probing on `&&&Lit<T>` takes the first
//!   impl whose bound holds: `16.0_f64` when `T: From<f64>`, else
//!   `16.0_f32` when `T: From<f32>`. f64 first is what inference already
//!   picks whenever it type-checks without the fallback (`f64` fields,
//!   types that take both); the fallback only ever chose `f32` when
//!   `f64` did not fit, so no call site changes meaning.
//! - The last level carries its bound on the METHOD (`where f64:
//!   Into<T>`), which probing does not check, so a field taking neither
//!   reports "`f64: Into<Field>` is not satisfied" at the literal rather
//!   than "no method `__lit`".
//!
//! The suffixed literals are the author's own digits (not an `as` cast
//! of one parse), so the `f32` value is exactly what `16.0_f32` means.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Expr, ExprLit, ExprUnary, Lit, LitFloat, UnOp};

/// The field-initializer tokens for a prop value whose field type pins the
/// conversion: `(value).into()`, or the suffix-picking block for an
/// unsuffixed float literal (optionally negated).
pub(crate) fn coerce_prop_value(value: &Expr) -> TokenStream2 {
    match unsuffixed_float(value) {
        Some((neg, lit)) => pick_float(neg, lit),
        None => quote! { (#value).into() },
    }
}

/// `16.0` / `-16.0` with no suffix, seen through the invisible groups a
/// `macro_rules!` fragment arrives in and through parentheses.
fn unsuffixed_float(value: &Expr) -> Option<(bool, &LitFloat)> {
    match value {
        Expr::Lit(ExprLit { lit: Lit::Float(f), .. }) if f.suffix().is_empty() => Some((false, f)),
        Expr::Unary(ExprUnary { op: UnOp::Neg(_), expr, .. }) => match unsuffixed_float(expr)? {
            (false, f) => Some((true, f)),
            // `--16.0` is not a literal shape worth special-casing.
            (true, _) => None,
        },
        Expr::Group(g) => unsuffixed_float(&g.expr),
        Expr::Paren(p) => unsuffixed_float(&p.expr),
        _ => None,
    }
}

fn suffixed(lit: &LitFloat, suffix: &str) -> LitFloat {
    let mut digits = lit.base10_digits().to_string();
    // `16.` + `f32` would lex as a field access on `16`.
    if digits.ends_with('.') {
        digits.push('0');
    }
    LitFloat::new(&format!("{digits}{suffix}"), lit.span())
}

fn pick_float(neg: bool, lit: &LitFloat) -> TokenStream2 {
    let sign = if neg { quote!(-) } else { quote!() };
    let l64 = suffixed(lit, "f64");
    let l32 = suffixed(lit, "f32");
    quote! {{
        struct __IdealystFloatLit<T>(::core::marker::PhantomData<T>);
        trait __IdealystAsF64<T> { fn __lit(&self) -> T; }
        impl<T: ::core::convert::From<f64>> __IdealystAsF64<T> for &&__IdealystFloatLit<T> {
            fn __lit(&self) -> T { <T as ::core::convert::From<f64>>::from(#sign #l64) }
        }
        trait __IdealystAsF32<T> { fn __lit(&self) -> T; }
        impl<T: ::core::convert::From<f32>> __IdealystAsF32<T> for &__IdealystFloatLit<T> {
            fn __lit(&self) -> T { <T as ::core::convert::From<f32>>::from(#sign #l32) }
        }
        trait __IdealystFloatInto<T> {
            fn __lit(&self) -> T where f64: ::core::convert::Into<T>;
        }
        impl<T> __IdealystFloatInto<T> for __IdealystFloatLit<T> {
            fn __lit(&self) -> T where f64: ::core::convert::Into<T> {
                ::core::convert::Into::into(#sign #l64)
            }
        }
        fn __pick<T>(f: fn(::core::marker::PhantomData<T>) -> T) -> T {
            f(::core::marker::PhantomData)
        }
        __pick(|__m| (&&&__IdealystFloatLit(__m)).__lit())
    }}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emit(src: &str) -> String {
        let e: Expr = syn::parse_str(src).unwrap();
        coerce_prop_value(&e).to_string().replace(' ', "")
    }

    #[test]
    fn non_float_values_keep_into() {
        assert_eq!(emit("16"), "(16).into()");
        assert_eq!(emit("\"x\""), "(\"x\").into()");
        assert_eq!(emit("16.0f32"), "(16.0f32).into()");
        assert_eq!(emit("width"), "(width).into()");
    }

    #[test]
    fn unsuffixed_float_gets_both_suffixed_spellings() {
        let out = emit("16.5");
        assert!(out.contains("from(16.5f64)"), "{out}");
        assert!(out.contains("from(16.5f32)"), "{out}");
        assert!(!out.contains(".into()"), "{out}");
    }

    #[test]
    fn negated_and_trailing_dot_floats() {
        let out = emit("-0.25");
        assert!(out.contains("from(-0.25f64)"), "{out}");
        let out = emit("(16.)");
        assert!(out.contains("from(16.0f32)"), "{out}");
    }
}
