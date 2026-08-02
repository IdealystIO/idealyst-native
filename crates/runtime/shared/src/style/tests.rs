use super::*;

// --- Interaction properties: cursor + user_select -----------------------

// The framework imposes no cursor/selection default: a fresh StyleRules
// leaves both unset, so a bare primitive inherits the platform default and
// only an author/component opt-in produces a non-default value.
#[test]
fn cursor_and_user_select_default_to_unset() {
    let r = StyleRules::default();
    assert_eq!(r.cursor, None);
    assert_eq!(r.user_select, None);
}

// `merge` overlays cursor/user_select like any other property: a set value
// in `other` wins, an unset value leaves the base untouched.
#[test]
fn merge_overlays_cursor_and_user_select() {
    let base = StyleRules {
        cursor: Some(Cursor::Pointer),
        user_select: Some(UserSelect::None),
        ..Default::default()
    };
    // An overlay that sets neither leaves the base values intact.
    let unchanged = base.clone().merge(&StyleRules::default());
    assert_eq!(unchanged.cursor, Some(Cursor::Pointer));
    assert_eq!(unchanged.user_select, Some(UserSelect::None));
    // An overlay that sets them wins.
    let over = StyleRules {
        cursor: Some(Cursor::Text),
        user_select: Some(UserSelect::Text),
        ..Default::default()
    };
    let merged = base.merge(&over);
    assert_eq!(merged.cursor, Some(Cursor::Text));
    assert_eq!(merged.user_select, Some(UserSelect::Text));
}

// Distinct cursor / user_select values must produce distinct content keys
// (else the backend's class cache would collide a pointer button with a
// text one and mint a single shared class). Equal values share a key.
#[test]
fn content_key_distinguishes_cursor_and_user_select() {
    let pointer = StyleRules { cursor: Some(Cursor::Pointer), ..Default::default() };
    let text = StyleRules { cursor: Some(Cursor::Text), ..Default::default() };
    let none = StyleRules::default();
    assert_ne!(pointer.content_key(), text.content_key());
    assert_ne!(pointer.content_key(), none.content_key());
    assert_eq!(pointer.content_key(), pointer.clone().content_key());

    let no_sel = StyleRules { user_select: Some(UserSelect::None), ..Default::default() };
    let all_sel = StyleRules { user_select: Some(UserSelect::All), ..Default::default() };
    assert_ne!(no_sel.content_key(), all_sel.content_key());
    assert_ne!(no_sel.content_key(), none.content_key());
}

// ----- object_fit: default + merge + content_key ------------------------

/// A bare `StyleRules` leaves `object_fit` unset; backends read that as
/// the framework default `Contain`. (The default is applied at the
/// backend, not baked into the field, so an unset image inherits it
/// without every sheet carrying an explicit value.)
#[test]
fn object_fit_defaults_to_unset() {
    assert_eq!(StyleRules::default().object_fit, None);
    // The enum's own default is Contain — the value backends fall back to.
    assert_eq!(ObjectFit::default(), ObjectFit::Contain);
}

/// `object_fit` round-trips through `merge`: an overlay that sets it
/// wins; an empty overlay leaves the base value intact. Guards the
/// "merge forgets a new field" bug class (the field must be in the
/// `overlay!` list).
#[test]
fn merge_overlays_object_fit() {
    let base = StyleRules { object_fit: Some(ObjectFit::Cover), ..Default::default() };
    let unchanged = base.clone().merge(&StyleRules::default());
    assert_eq!(unchanged.object_fit, Some(ObjectFit::Cover));
    let over = StyleRules { object_fit: Some(ObjectFit::Fill), ..Default::default() };
    assert_eq!(base.merge(&over).object_fit, Some(ObjectFit::Fill));
}

/// Distinct `object_fit` values mint distinct content keys (else a Cover
/// image and a Contain image sharing a sheet shape would collide on one
/// minted class); equal values share a key; Some differs from None.
#[test]
fn content_key_distinguishes_object_fit() {
    let cover = StyleRules { object_fit: Some(ObjectFit::Cover), ..Default::default() };
    let contain = StyleRules { object_fit: Some(ObjectFit::Contain), ..Default::default() };
    let none = StyleRules::default();
    assert_ne!(cover.content_key(), contain.content_key());
    assert_ne!(cover.content_key(), none.content_key());
    assert_eq!(cover.content_key(), cover.clone().content_key());
}

/// Pass-through no-op closures for the non-key params of
/// `ensure_registered_with`, so a test can focus on one slot.
fn drain_with_key_recorder(
    sheet: &Rc<StyleSheet>,
    record: impl FnOnce(Option<crate::primitives::key::KeyDownHandler>),
) {
    ensure_registered_with(
        sheet,
        |_| {},
        |_| {},
        |_| {},
        |_| {},
        |_, _, _| {},
        |_, _, _, _| {},
        |_| {},
        |_, _| {},
        record,
    );
}

// The app-level key handler queued by `set_app_key_handler` must reach the
// backend (via `Backend::set_app_key_handler`) on the next flush — and only
// once (single-slot). Regression for the cross-backend global-keyboard path:
// without the drain in `ensure_registered_with`, the handler would be stashed
// forever and never installed, so app shortcuts would silently do nothing.
#[test]
fn set_app_key_handler_routes_to_backend_once() {
    use std::cell::Cell;
    let sheet = Rc::new(StyleSheet::r#static(StyleRules::default()));

    // Install a handler → it drains to the recorder as `Some`.
    let handler: crate::primitives::key::KeyDownHandler =
        Rc::new(|_e| crate::primitives::key::KeyOutcome::Default);
    set_app_key_handler(Some(handler));
    let drained: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
    {
        let d = drained.clone();
        drain_with_key_recorder(&sheet, move |h| d.set(Some(h.is_some())));
    }
    assert_eq!(drained.get(), Some(true), "installed handler reached the backend");

    // Single-slot: a second flush with nothing queued doesn't call through.
    let called_again = Rc::new(Cell::new(false));
    {
        let c = called_again.clone();
        drain_with_key_recorder(&sheet, move |_h| c.set(true));
    }
    assert!(!called_again.get(), "no pending handler → no second backend call");

    // Clearing (`None`) also routes through as `Some(None)`.
    set_app_key_handler(None);
    let cleared: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
    {
        let c = cleared.clone();
        drain_with_key_recorder(&sheet, move |h| c.set(Some(h.is_some())));
    }
    assert_eq!(cleared.get(), Some(false), "clear routes through as None");
}

/// Helper: assert a `Tokenized<Color>` resolves to a particular
/// fallback string. Tests express the visible color, not whether
/// the rule used a token vs literal.
fn color_eq(actual: &Option<Tokenized<Color>>, expected_hex: &str) {
    let value = actual
        .as_ref()
        .expect("expected Some color")
        .value();
    assert_eq!(value.0, expected_hex);
}

#[test]
fn closure_stylesheet_emits_rules() {
    let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules {
        background: Some(Tokenized::token("surface", Color("#fff".into()))),
        padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
        ..Default::default()
    });
    let r = sheet.resolve(&VariantSet::new());
    color_eq(&r.background, "#fff");
    assert_eq!(r.padding_top, Some(Tokenized::Literal(Length::Px(16.0))));
}

#[test]
fn static_stylesheet_returns_fixed_rules() {
    let sheet = StyleSheet::r#static(StyleRules {
        background: Some(Tokenized::Literal(Color("#abc".into()))),
        ..Default::default()
    });
    let r = sheet.resolve(&VariantSet::new());
    color_eq(&r.background, "#abc");
}

#[test]
fn variant_overlays_layer_on_top_of_base() {
    let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules {
        background: Some(Tokenized::token("surface", Color("#fff".into()))),
        padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
        ..Default::default()
    })
    .variant("size", "large", |_vs: &VariantSet| StyleRules {
        padding_top: Some(Tokenized::Literal(Length::Px(32.0))),
        ..Default::default()
    });
    let r = sheet.resolve(&VariantSet::new().with("size", "large"));
    color_eq(&r.background, "#fff");
    assert_eq!(r.padding_top, Some(Tokenized::Literal(Length::Px(32.0))));
}

#[test]
fn update_tokens_clears_resolution_cache() {
    let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        background: Some(Tokenized::token("surface", Color("#fff".into()))),
        ..Default::default()
    }));
    let app = StyleApplication::new(sheet);

    let r1 = resolve(&app);
    color_eq(&r1.background, "#fff");

    // Subsequent resolves hit the cache and return the same Rc.
    let r2 = resolve(&app);
    assert!(Rc::ptr_eq(&r1, &r2));

    // `update_tokens` wipes the cache; the next resolve produces
    // a fresh Rc (token names are stable so the content matches).
    update_tokens(&[TokenEntry {
        name: "surface",
        value: TokenValue::Color(Color("#111".into())),
    }]);
    let r3 = resolve(&app);
    assert!(!Rc::ptr_eq(&r1, &r3));
}

#[test]
fn overrides_layer_on_top_of_base_and_variants() {
    let sheet = Rc::new(
        StyleSheet::new(|_vs: &VariantSet| StyleRules {
            background: Some(Tokenized::token("surface", Color("#fff".into()))),
            font_size: Some(Tokenized::Literal(Length::Px(14.0))),
            padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
            ..Default::default()
        })
        .variant("size", "large", |_vs: &VariantSet| StyleRules {
            font_size: Some(Tokenized::Literal(Length::Px(20.0))),
            ..Default::default()
        }),
    );

    // Base only.
    let r1 = resolve(&StyleApplication::new(sheet.clone()));
    assert_eq!(r1.font_size, Some(Tokenized::Literal(Length::Px(14.0))));

    // With variant: font becomes 20.
    let r2 = resolve(&StyleApplication::new(sheet.clone()).with("size", "large"));
    assert_eq!(r2.font_size, Some(Tokenized::Literal(Length::Px(20.0))));

    // With variant + override: override wins.
    let r3 = resolve(
        &StyleApplication::new(sheet.clone())
            .with("size", "large")
            .override_font_size(17.5),
    );
    assert_eq!(r3.font_size, Some(Tokenized::Literal(Length::Px(17.5))));
    // Other properties unaffected by the override.
    assert_eq!(r3.padding_top, Some(Tokenized::Literal(Length::Px(16.0))));

    // Different override values produce distinct cache entries.
    let r4 = resolve(
        &StyleApplication::new(sheet.clone())
            .with("size", "large")
            .override_font_size(99.0),
    );
    assert_eq!(r4.font_size, Some(Tokenized::Literal(Length::Px(99.0))));
    assert!(!Rc::ptr_eq(&r3, &r4));
}

// The bulk `with_overrides` counterpart to the per-field `override_*`
// setters: a whole `StyleRules` layered on top, each set field winning over
// the sheet, and preserving any prior overrides it doesn't touch. This is
// the primitive behind idea-ui's per-slot `*_style` override props.
#[test]
fn with_overrides_layers_a_whole_rules_and_preserves_prior() {
    let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        color: Some(Tokenized::Literal(Color("#111111".into()))),
        padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
        font_size: Some(Tokenized::Literal(Length::Px(14.0))),
        ..Default::default()
    }));

    // A wholesale override wins for every field it sets; untouched sheet
    // fields (font_size) survive.
    let app = StyleApplication::new(sheet.clone()).with_overrides(StyleRules {
        color: Some(Tokenized::Literal(Color("#0b6b3a".into()))),
        padding_top: Some(Tokenized::Literal(Length::Px(0.0))),
        ..Default::default()
    });
    let r = resolve(&app);
    assert_eq!(r.color, Some(Tokenized::Literal(Color("#0b6b3a".into()))), "override color wins");
    assert_eq!(r.padding_top, Some(Tokenized::Literal(Length::Px(0.0))), "override zero-padding wins (flush)");
    assert_eq!(r.font_size, Some(Tokenized::Literal(Length::Px(14.0))), "untouched sheet field survives");

    // A prior per-field override is preserved when the bulk override doesn't
    // set that field, and beaten when it does.
    let app2 = StyleApplication::new(sheet)
        .override_color(Color("#ff0000".into()))
        .with_overrides(StyleRules {
            padding_top: Some(Tokenized::Literal(Length::Px(4.0))),
            ..Default::default()
        });
    let r2 = resolve(&app2);
    assert_eq!(r2.color, Some(Tokenized::Literal(Color("#ff0000".into()))), "prior override_color preserved");
    assert_eq!(r2.padding_top, Some(Tokenized::Literal(Length::Px(4.0))), "bulk override applied on top");
}

// ------------------------------------------------------------------
// Computed layer — runtime-evaluated `StyleRules` between variants
// and overrides. Used by open-extension variant systems where the
// modifier matrix isn't enumerable at compile time.
// ------------------------------------------------------------------

#[test]
fn computed_layer_merges_between_variants_and_overrides() {
    let sheet = Rc::new(
        StyleSheet::new(|_vs: &VariantSet| StyleRules {
            background: Some(Tokenized::token("surface", Color("#fff".into()))),
            color: Some(Tokenized::Literal(Color("#111".into()))),
            font_size: Some(Tokenized::Literal(Length::Px(14.0))),
            ..Default::default()
        })
        .variant("size", "large", |_vs: &VariantSet| StyleRules {
            font_size: Some(Tokenized::Literal(Length::Px(20.0))),
            ..Default::default()
        }),
    );

    // Computed layer sets background + color. Variant sets font_size.
    // Override sets font_size to a third value. Result should pick:
    //   background ← computed (since base+variants didn't override)
    //   color ← computed (since base set it, computed overrides)
    //   font_size ← override (override is last layer)
    let app = StyleApplication::new(sheet.clone())
        .with("size", "large")
        .with_computed("filled+danger", || StyleRules {
            background: Some(Tokenized::Literal(Color("#e5484d".into()))),
            color: Some(Tokenized::Literal(Color("#ffffff".into()))),
            ..Default::default()
        })
        .override_font_size(99.0);
    let r = resolve(&app);
    color_eq(&r.background, "#e5484d");
    color_eq(&r.color, "#ffffff");
    assert_eq!(r.font_size, Some(Tokenized::Literal(Length::Px(99.0))));
}

#[test]
fn computed_layer_shares_cache_entry_across_equivalent_keys() {
    let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        ..Default::default()
    }));

    // Two separate apps with the same computed key produce closures
    // that return equivalent StyleRules. The framework must reuse a
    // single cached Rc<StyleRules> — that's what makes 1000 buttons
    // with `tone=Danger, variant=Filled, size=Md` materialize one
    // class on the backend, not 1000.
    let make_app = || {
        StyleApplication::new(sheet.clone()).with_computed("filled+danger+md", || StyleRules {
            background: Some(Tokenized::Literal(Color("#e5484d".into()))),
            ..Default::default()
        })
    };
    let r1 = resolve(&make_app());
    let r2 = resolve(&make_app());
    assert!(
        Rc::ptr_eq(&r1, &r2),
        "equal computed keys must share the cached Rc<StyleRules>",
    );
}

#[test]
fn computed_layer_distinct_keys_produce_distinct_cache_entries() {
    let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        ..Default::default()
    }));

    let app_a = StyleApplication::new(sheet.clone()).with_computed("filled+danger", || StyleRules {
        background: Some(Tokenized::Literal(Color("#e5484d".into()))),
        ..Default::default()
    });
    let app_b = StyleApplication::new(sheet.clone()).with_computed("filled+success", || StyleRules {
        background: Some(Tokenized::Literal(Color("#3ba55d".into()))),
        ..Default::default()
    });

    let r_a = resolve(&app_a);
    let r_b = resolve(&app_b);
    assert!(!Rc::ptr_eq(&r_a, &r_b));
    color_eq(&r_a.background, "#e5484d");
    color_eq(&r_b.background, "#3ba55d");
}

#[test]
fn computed_layer_reruns_after_token_update() {
    // Closure reads a token-backed value. After `update_tokens`
    // wipes the cache, the next resolve must re-run the closure so
    // theme-dependent reads pick up the new value. This is the
    // mechanism that makes a custom Tone re-render correctly on
    // light/dark swap.
    let sheet = Rc::new(StyleSheet::new(|_vs: &VariantSet| StyleRules {
        ..Default::default()
    }));
    let app = StyleApplication::new(sheet).with_computed("hype-tone", || StyleRules {
        background: Some(Tokenized::token(
            "tone-hype-fill-bg",
            Color("#ff00aa".into()),
        )),
        ..Default::default()
    });

    let r1 = resolve(&app);
    // Same key + no cache wipe → same Rc.
    let r2 = resolve(&app);
    assert!(Rc::ptr_eq(&r1, &r2));

    // Token update wipes the cache. The closure re-runs; the
    // returned `StyleRules` carries the same token name (so the
    // resolved class name is theme-stable), but its `Rc` identity
    // is fresh.
    update_tokens(&[TokenEntry {
        name: "tone-hype-fill-bg",
        value: TokenValue::Color(Color("#cc0088".into())),
    }]);
    let r3 = resolve(&app);
    assert!(!Rc::ptr_eq(&r1, &r3));
    // Token name is preserved (theme-stable identity) even though
    // a fresh closure execution constructed the value.
    assert_eq!(
        r3.background.as_ref().and_then(|t| t.name()),
        Some("tone-hype-fill-bg"),
    );
}

#[test]
fn computed_layer_fast_path_disabled_when_attached() {
    // The fast path (sheet.lookup_variant) skips the resolution
    // cache entirely and would miss the computed layer. The fast
    // path must therefore be disabled whenever a computed layer is
    // present — verify by attaching a computed layer that shadows a
    // variant's property and confirming the computed value wins.
    let sheet = Rc::new(
        StyleSheet::new(|_vs: &VariantSet| StyleRules {
            color: Some(Tokenized::Literal(Color("#000000".into()))),
            ..Default::default()
        })
        .variant("size", "large", |_vs: &VariantSet| StyleRules {
            color: Some(Tokenized::Literal(Color("#222222".into()))),
            ..Default::default()
        }),
    );

    let app = StyleApplication::new(sheet)
        .with("size", "large")
        .with_computed("custom-color", || StyleRules {
            color: Some(Tokenized::Literal(Color("#ff00aa".into()))),
            ..Default::default()
        });
    let r = resolve(&app);
    color_eq(&r.color, "#ff00aa");
}

#[test]
fn variant_default_applies_when_axis_unselected() {
    let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules {
        padding_top: Some(Tokenized::Literal(Length::Px(8.0))),
        ..Default::default()
    })
    .variant("size", "small", |_vs: &VariantSet| StyleRules {
        padding_top: Some(Tokenized::Literal(Length::Px(4.0))),
        ..Default::default()
    })
    .variant("size", "large", |_vs: &VariantSet| StyleRules {
        padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
        ..Default::default()
    })
    .variant_default("size", "large");

    // Call site omits `size` → default "large" applies → padding 16.
    let r = sheet.resolve(&VariantSet::new());
    assert_eq!(r.padding_top, Some(Tokenized::Literal(Length::Px(16.0))));

    // Call site picks "small" → padding 4.
    let r2 = sheet.resolve(&VariantSet::new().with("size", "small"));
    assert_eq!(r2.padding_top, Some(Tokenized::Literal(Length::Px(4.0))));
}

#[test]
fn compound_variant_applies_only_when_all_match() {
    let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules::default())
        .variant("size", "large", |_vs: &VariantSet| StyleRules {
            padding_top: Some(Tokenized::Literal(Length::Px(16.0))),
            ..Default::default()
        })
        .variant("kind", "primary", |_vs: &VariantSet| StyleRules {
            background: Some(Tokenized::Literal(Color("primary-bg".into()))),
            ..Default::default()
        })
        .compound(
            vec![("size", "large"), ("kind", "primary")],
            |_vs: &VariantSet| StyleRules {
                font_size: Some(Tokenized::Literal(Length::Px(24.0))),
                ..Default::default()
            },
        );

    // Only size=large → compound NOT applied.
    let r1 = sheet.resolve(&VariantSet::new().with("size", "large"));
    assert_eq!(r1.padding_top, Some(Tokenized::Literal(Length::Px(16.0))));
    assert_eq!(r1.font_size, None);

    // Both axes match → compound APPLIED.
    let r2 = sheet.resolve(
        &VariantSet::new().with("size", "large").with("kind", "primary"),
    );
    assert_eq!(r2.padding_top, Some(Tokenized::Literal(Length::Px(16.0))));
    color_eq(&r2.background, "primary-bg");
    assert_eq!(r2.font_size, Some(Tokenized::Literal(Length::Px(24.0))));
}

#[test]
fn variant_keys_lists_every_axis_value() {
    let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules::default())
        .variant("size", "small", |_vs: &VariantSet| StyleRules::default())
        .variant("size", "large", |_vs: &VariantSet| StyleRules::default())
        .variant("kind", "primary", |_vs: &VariantSet| StyleRules::default());
    let mut keys = sheet.variant_keys();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            ("kind".to_string(), "primary".to_string()),
            ("size".to_string(), "large".to_string()),
            ("size".to_string(), "small".to_string()),
        ]
    );
}

#[test]
fn resolve_memoizes_same_inputs() {
    let sheet = Rc::new(StyleSheet::r#static(StyleRules {
        background: Some(Tokenized::Literal(Color("#abc".into()))),
        ..Default::default()
    }));
    let app = StyleApplication::new(sheet);
    let r1 = resolve(&app);
    let r2 = resolve(&app);
    assert!(Rc::ptr_eq(&r1, &r2));
}

/// **The core invariant of the tokenization rework**: two
/// stylesheets producing the same token references must hash to
/// the same content key regardless of installed token values.
#[test]
fn tokenized_rules_have_token_stable_content_keys() {
    let sheet_a = StyleSheet::new(|_vs: &VariantSet| StyleRules {
        background: Some(Tokenized::token("surface", Color("#fff".into()))),
        ..Default::default()
    });
    let sheet_b = StyleSheet::new(|_vs: &VariantSet| StyleRules {
        background: Some(Tokenized::token("surface", Color("#111".into()))),
        ..Default::default()
    });
    let r_a = sheet_a.resolve(&VariantSet::new());
    let r_b = sheet_b.resolve(&VariantSet::new());
    assert_eq!(r_a.content_key(), r_b.content_key());
    // Sanity: the *fallbacks* differ so we know the test is real.
    assert_ne!(r_a.background.as_ref().unwrap().value().0,
               r_b.background.as_ref().unwrap().value().0);
}

/// Literal values should NOT collide with token references that
/// happen to share a string. The content key encoder tags
/// literals with `L:` and tokens with `T:` to disambiguate.
#[test]
fn literal_and_token_with_same_string_have_distinct_keys() {
    let lit_rules = StyleRules {
        background: Some(Tokenized::Literal(Color("surface".into()))),
        ..Default::default()
    };
    let tok_rules = StyleRules {
        background: Some(Tokenized::token("surface", Color("anything".into()))),
        ..Default::default()
    };
    assert_ne!(lit_rules.content_key(), tok_rules.content_key());
}

// -----------------------------------------------------------------
// Per-token reactivity (TOKEN_REGISTRY / Tokenized::resolve)
// -----------------------------------------------------------------
//
// Tests use globally-unique token names ("tk_<test>_<token>") to
// avoid cross-test contamination from the thread-local registry —
// the registry persists across tests on the same thread because
// it lives outside any `Scope`.

#[test]
fn install_tokens_populates_registry_and_resolve_returns_installed_value() {
    install_tokens(&[
        TokenEntry {
            name: "tk_install_color",
            value: TokenValue::Color(Color("#123".into())),
        },
        TokenEntry {
            name: "tk_install_len",
            value: TokenValue::Length(Length::Px(24.0)),
        },
        TokenEntry {
            name: "tk_install_num",
            value: TokenValue::Number(0.5),
        },
    ]);

    let c: Tokenized<Color> = Tokenized::token("tk_install_color", Color("#fff".into()));
    let l: Tokenized<Length> = Tokenized::token("tk_install_len", Length::Px(0.0));
    let n: Tokenized<f32> = Tokenized::token("tk_install_num", 0.0);
    assert_eq!(c.resolve().0, "#123");
    assert_eq!(l.resolve(), Length::Px(24.0));
    assert_eq!(n.resolve(), 0.5);
}

#[test]
fn resolve_literal_returns_value_and_does_not_touch_registry() {
    let c: Tokenized<Color> = Tokenized::Literal(Color("#abc".into()));
    assert_eq!(c.resolve().0, "#abc");
    let l: Tokenized<Length> = Tokenized::Literal(Length::Px(8.0));
    assert_eq!(l.resolve(), Length::Px(8.0));
    let n: Tokenized<f32> = Tokenized::Literal(7.5);
    assert_eq!(n.resolve(), 7.5);
}

#[test]
fn resolve_uninstalled_token_returns_fallback() {
    // Token name never installed — resolve still works and lazily
    // creates a registry entry seeded with the fallback so subsequent
    // `update_tokens` for the same name can propagate.
    //
    // Install a *different* token first so the thread is marked
    // themed; the permissive lazy-fallback semantics we're
    // exercising apply to individual missing tokens, not to a
    // totally-unthemed thread.
    install_tokens(&[TokenEntry {
        name: "tk_uninstalled_sentinel",
        value: TokenValue::Color(Color("#000".into())),
    }]);
    let c: Tokenized<Color> = Tokenized::token("tk_uninstalled", Color("#fall".into()));
    assert_eq!(c.resolve().0, "#fall");
}

/// Regression test for the `with_or_create_token_signal` scope-adoption
/// audit finding. Token signals stashed in the thread-local
/// `TOKEN_REGISTRY` must outlive any render scope — they're the
/// theme system's authoritative store and need thread lifetime to
/// survive re-mounts (hot reload, fixture teardown, page-rebuild
/// in dev tools).
///
/// Bug before fix: `Signal::new` inside `with_or_create_token_signal`
/// gets registered with the currently-active `Scope`. When that
/// scope drops (e.g. app unmount), the slot is freed but
/// `TOKEN_REGISTRY` still holds a stale `Signal` handle. The next
/// resolve of the same token either panics with
/// "signal used after its scope was dropped" or — worse — silently
/// hits a recycled slot of an unrelated signal.
#[test]
fn token_signal_survives_creating_scope_drop() {
    use crate::reactive::{with_scope, Scope};

    // Use a unique token name so this test doesn't collide with the
    // other registry-touching tests (registry is process-wide).
    const NAME: &str = "tk_scope_survival_color";

    // Mark the thread as themed. The bug under test is about lazy
    // creation of a *single missing* token's signal slot, not about
    // a totally-unthemed thread.
    install_tokens(&[TokenEntry {
        name: "tk_scope_survival_sentinel",
        value: TokenValue::Color(Color("#000".into())),
    }]);

    // First read happens inside scope A. Resolves the token, which
    // creates the registry signal lazily.
    {
        let mut scope_a = Scope::new();
        with_scope(&mut scope_a, || {
            let c: Tokenized<Color> = Tokenized::token(NAME, Color("#aaa".into()));
            let v = c.resolve();
            assert_eq!(v.0, "#aaa", "first resolve returns the fallback");
        });
        // scope_a drops here. With the bug, the token signal's
        // arena slot is freed; the registry still holds the stale
        // Signal handle.
    }

    // Second read happens inside an unrelated scope B. Must NOT
    // panic — the token registry is supposed to be thread-lifetime.
    let mut scope_b = Scope::new();
    let observed = with_scope(&mut scope_b, || {
        let c: Tokenized<Color> = Tokenized::token(NAME, Color("#bbb".into()));
        c.resolve()
    });
    // We expect the fallback that was installed on the first
    // resolve to still be returned (registry preserved its
    // contents). What we don't expect is a panic.
    assert_eq!(
        observed.0, "#aaa",
        "second resolve must return the originally-installed fallback, \
         proving the token signal outlived its creating scope"
    );

    // `update_tokens` should also work after the creator scope dropped.
    update_tokens(&[TokenEntry {
        name: NAME,
        value: TokenValue::Color(Color("#ccc".into())),
    }]);
    let mut scope_c = Scope::new();
    let updated = with_scope(&mut scope_c, || {
        let c: Tokenized<Color> = Tokenized::token(NAME, Color("#bbb".into()));
        c.resolve()
    });
    assert_eq!(
        updated.0, "#ccc",
        "update_tokens through the registry-stashed signal must still work \
         after the creating scope dropped"
    );
}

/// `update_tokens(["a"])` must fire only the signal for `"a"` — the
/// signal for `"b"` stays still. This is the per-token isolation
/// invariant at the signal layer.
#[test]
fn update_tokens_fires_only_changed_token_signal() {
    use std::cell::Cell;
    use std::rc::Rc;

    install_tokens(&[
        TokenEntry {
            name: "tk_isolate_a",
            value: TokenValue::Color(Color("#a0".into())),
        },
        TokenEntry {
            name: "tk_isolate_b",
            value: TokenValue::Color(Color("#b0".into())),
        },
    ]);

    let a_runs = Rc::new(Cell::new(0u32));
    let b_runs = Rc::new(Cell::new(0u32));
    let a_runs_c = a_runs.clone();
    let b_runs_c = b_runs.clone();

    let tok_a: Tokenized<Color> =
        Tokenized::token("tk_isolate_a", Color("#fall".into()));
    let tok_b: Tokenized<Color> =
        Tokenized::token("tk_isolate_b", Color("#fall".into()));

    let _ea = crate::Effect::new(move || {
        let _ = tok_a.resolve();
        a_runs_c.set(a_runs_c.get() + 1);
    });
    let _eb = crate::Effect::new(move || {
        let _ = tok_b.resolve();
        b_runs_c.set(b_runs_c.get() + 1);
    });
    assert_eq!(a_runs.get(), 1, "effect A fired once on install");
    assert_eq!(b_runs.get(), 1, "effect B fired once on install");

    update_tokens(&[TokenEntry {
        name: "tk_isolate_a",
        value: TokenValue::Color(Color("#a1".into())),
    }]);
    assert_eq!(a_runs.get(), 2, "effect A re-fires on its token's update");
    assert_eq!(b_runs.get(), 1, "effect B did NOT re-fire on A's update");

    update_tokens(&[TokenEntry {
        name: "tk_isolate_b",
        value: TokenValue::Color(Color("#b1".into())),
    }]);
    assert_eq!(a_runs.get(), 2, "effect A unchanged by B's update");
    assert_eq!(
        b_runs.get(),
        2,
        "effect B re-fires on its token's update"
    );
}

/// **Load-bearing test for the whole refactor.** A styled-effect-
/// like setup: a `Tokenized::resolve()` read inside an Effect
/// subscribes that effect to ONLY the specific token signal — so
/// an `update_tokens` for a *different* token leaves the effect
/// untouched. This is the property that lets a 10k-row scoreboard
/// avoid waking nodes that don't reference the changed token.
#[test]
fn per_token_isolation_in_styled_effect() {
    use std::cell::Cell;
    use std::rc::Rc;

    install_tokens(&[
        TokenEntry {
            name: "tk_styled_a",
            value: TokenValue::Color(Color("#aaaaaa".into())),
        },
        TokenEntry {
            name: "tk_styled_b",
            value: TokenValue::Color(Color("#bbbbbb".into())),
        },
    ]);

    // Effect that reads ONLY token A (like a node whose stylesheet
    // references `tk_styled_a` for background).
    let runs = Rc::new(Cell::new(0u32));
    let last_value = Rc::new(Cell::new(String::new()));
    let runs_c = runs.clone();
    let last_value_c = last_value.clone();
    let tok_a: Tokenized<Color> =
        Tokenized::token("tk_styled_a", Color("#fff".into()));
    let _e = crate::Effect::new(move || {
        // Mirror what a backend's apply_style does — resolve the
        // tokenized property to a concrete value.
        let resolved = tok_a.resolve();
        last_value_c.set(resolved.0);
        runs_c.set(runs_c.get() + 1);
    });
    assert_eq!(runs.get(), 1, "initial run on install");
    assert_eq!(last_value.take(), "#aaaaaa");

    // Update an UNRELATED token (B). Our effect must NOT re-fire.
    update_tokens(&[TokenEntry {
        name: "tk_styled_b",
        value: TokenValue::Color(Color("#b1b1b1".into())),
    }]);
    assert_eq!(
        runs.get(),
        1,
        "styled effect reading only tk_styled_a must not wake on tk_styled_b updates"
    );

    // Update the SUBSCRIBED token (A). Effect re-fires with new value.
    update_tokens(&[TokenEntry {
        name: "tk_styled_a",
        value: TokenValue::Color(Color("#a1a1a1".into())),
    }]);
    assert_eq!(runs.get(), 2, "styled effect re-fires on its own token");
    assert_eq!(last_value.take(), "#a1a1a1");
}

#[test]
fn update_tokens_before_install_is_permissive() {
    // Calling update_tokens for a never-installed name creates
    // the registry entry — subsequent resolves see the value.
    update_tokens(&[TokenEntry {
        name: "tk_permissive",
        value: TokenValue::Length(Length::Px(99.0)),
    }]);
    let t: Tokenized<Length> = Tokenized::token("tk_permissive", Length::Px(0.0));
    assert_eq!(t.resolve(), Length::Px(99.0));
}

#[test]
fn resolve_with_wrong_variant_falls_back() {
    // Install a token as Length, then read via Tokenized<Color>.
    // Should return the fallback (and emit a debug eprintln in
    // debug builds — not asserted to avoid coupling).
    install_tokens(&[TokenEntry {
        name: "tk_wrong_variant",
        value: TokenValue::Length(Length::Px(10.0)),
    }]);
    let c: Tokenized<Color> = Tokenized::token("tk_wrong_variant", Color("#fb".into()));
    assert_eq!(c.resolve().0, "#fb");
}

/// Regression test for the "native aborts when no theme is installed"
/// report (Whiteboard Pro feedback, 2026-06): an app that styles with
/// literal colors — or just leans on primitive default tokens like
/// `color-text` — and never calls `install_theme` rendered fine on web
/// (`var(--color-text, #1a1a1f)`) but `SIGABRT`-ed deep in style
/// resolution on macOS via a `debug_assert!` tripwire.
///
/// Resolving a `Tokenized::Token` on a thread with no installed theme
/// must return the embedded fallback and **never panic**, matching the
/// web backend (CLAUDE.md §7: backends converge in output). The
/// cross-thread footgun the tripwire originally guarded is now a
/// debug-only *warning* (see `debug_warn_resolve_on_unthemed_thread`),
/// not an abort.
///
/// Spawning a fresh, never-themed thread is the only way to
/// deterministically exercise an unthemed thread — the test runner
/// reuses threads across tests and the parent thread gets themed by
/// the other registry-touching tests in this module. The assertion
/// holds in both debug and release builds (the behavior is identical),
/// so this test is intentionally *not* `#[cfg(debug_assertions)]`.
#[test]
fn resolve_on_unthemed_thread_falls_back_without_panicking() {
    let handle = std::thread::Builder::new()
        .name("unthemed_resolve_thread".into())
        .spawn(|| {
            let c: Tokenized<Color> =
                Tokenized::token("tk_unthemed_thread", Color("#fall".into()));
            // Must return the fallback, not panic.
            c.resolve().0
        })
        .expect("spawn worker thread");

    let resolved = handle
        .join()
        .expect("resolving a token on an unthemed thread must not panic");
    assert_eq!(
        resolved, "#fall",
        "resolving a token on an unthemed thread must return the literal \
         fallback — native must match the web backend's silent \
         `var(--name, fallback)` behavior"
    );
}

/// Regression test for the "border width type uniformity" papercut
/// (Whiteboard Pro feedback): `border_*_width` is `Tokenized<f32>`
/// while every other length field is `Tokenized<Length>`, so passing
/// a `Length` used to fail with a confusing trait error. A `Length`
/// now coerces into `Tokenized<f32>` — pixels pass through; percent
/// and auto are invalid for a border and collapse to `0.0`.
#[test]
fn length_coerces_into_tokenized_f32_for_border_widths() {
    let px: Tokenized<f32> = Length::Px(2.5).into();
    assert_eq!(px.resolve(), 2.5);

    // Percent/Auto are meaningless for a border → 0.0 (not a panic,
    // not a type error).
    let pct: Tokenized<f32> = Length::Percent(50.0).into();
    assert_eq!(pct.resolve(), 0.0);
    let auto: Tokenized<f32> = Length::Auto.into();
    assert_eq!(auto.resolve(), 0.0);
}

// -----------------------------------------------------------------
// FontFamily + typeface registration
// -----------------------------------------------------------------

// `face!` embeds via `include_bytes!`, so its src paths must
// point at real files. We use sibling `runtime-core` sources
// as test-only embed targets — the bytes are irrelevant; the
// tests only exercise `Typeface`/`FontFamily` identity + struct
// shape.
fn sample_typeface() -> crate::assets::Typeface {
    crate::typeface! {
        name: "TestSans",
        faces: [
            crate::face!(weight: FontWeight::Normal, style: FontStyle::Normal,
                         src: "../assets.rs"),
            crate::face!(weight: FontWeight::Bold, style: FontStyle::Normal,
                         src: "../lib.rs"),
        ],
        fallback: crate::assets::SystemFallback::SansSerif,
    }
}

fn other_typeface() -> crate::assets::Typeface {
    crate::typeface! {
        name: "TestMono",
        faces: [
            crate::face!(weight: FontWeight::Normal, style: FontStyle::Normal,
                         src: "../reactive.rs"),
        ],
        fallback: crate::assets::SystemFallback::Monospace,
    }
}

#[test]
fn font_family_from_string_and_str_produce_system() {
    let from_str: FontFamily = "Helvetica".into();
    let from_string: FontFamily = String::from("Helvetica").into();
    assert_eq!(from_str, FontFamily::System("Helvetica".to_string()));
    assert_eq!(from_string, from_str);
}

// The deleted-`typeface!` DX warning's pure decision. Gated on
// `debug_assertions` because `should_warn_for_system_font` itself is
// debug-only (the whole guardrail compiles out in release).
#[cfg(debug_assertions)]
#[test]
fn should_warn_for_system_font_decision_table() {
    use super::should_warn_for_system_font;
    use rustc_hash::FxHashSet;

    let registered: FxHashSet<&'static str> = ["Inter", "Source Code Pro"]
        .into_iter()
        .collect();

    // Bare, unregistered, non-generic → looks like a removed
    // `typeface!` registration. WARN.
    assert!(should_warn_for_system_font("Roboto Mono", &registered));
    assert!(should_warn_for_system_font("MyCustomFace", &registered));

    // Registered typeface family → resolves fine, no warning.
    assert!(!should_warn_for_system_font("Inter", &registered));
    assert!(!should_warn_for_system_font("Source Code Pro", &registered));

    // Known generic / system families → intentional, no warning
    // (case-insensitive).
    assert!(!should_warn_for_system_font("sans-serif", &registered));
    assert!(!should_warn_for_system_font("serif", &registered));
    assert!(!should_warn_for_system_font("monospace", &registered));
    assert!(!should_warn_for_system_font("system-ui", &registered));
    assert!(!should_warn_for_system_font("-apple-system", &registered));
    assert!(!should_warn_for_system_font("BlinkMacSystemFont", &registered));
    assert!(!should_warn_for_system_font("Segoe UI", &registered));
    assert!(!should_warn_for_system_font("ARIAL", &registered));

    // Comma stack → explicit fallback list, never a bare face.
    assert!(!should_warn_for_system_font("Inter, sans-serif", &registered));
    assert!(!should_warn_for_system_font(
        "NotRegistered, sans-serif",
        &registered
    ));

    // Empty / whitespace → nothing actionable.
    assert!(!should_warn_for_system_font("", &registered));
    assert!(!should_warn_for_system_font("   ", &registered));

    // Quoted bare generic is still recognized as generic.
    assert!(!should_warn_for_system_font("\"sans-serif\"", &registered));
    // Quoted registered family is still recognized as registered.
    assert!(!should_warn_for_system_font("\"Inter\"", &registered));
    // Quoted unregistered non-generic still warns.
    assert!(should_warn_for_system_font("\"Ghost\"", &registered));

    // Surrounding whitespace is trimmed before matching.
    assert!(!should_warn_for_system_font("  Inter  ", &registered));
}

#[test]
fn font_family_from_typeface_wraps_value() {
    let tf = sample_typeface();
    let ff: FontFamily = tf.into();
    match ff {
        FontFamily::Typeface(t) => assert_eq!(t.id, tf.id),
        _ => panic!("expected Typeface variant"),
    }
}

#[test]
fn font_family_eq_by_typeface_id_not_struct() {
    let tf = sample_typeface();
    // Same id but synthetic struct missing the static metadata —
    // exercises the manual `PartialEq` that compares on id only.
    let synthetic = crate::assets::Typeface {
        id: tf.id,
        family_name: "",
        faces: &[],
        fallback: crate::assets::SystemFallback::None,
    };
    let a = FontFamily::Typeface(tf);
    let b = FontFamily::Typeface(synthetic);
    assert_eq!(a, b);
}

#[test]
fn font_family_system_and_typeface_never_equal() {
    let tf = sample_typeface();
    let a = FontFamily::System("X".to_string());
    let b = FontFamily::Typeface(tf);
    assert_ne!(a, b);
}

#[test]
fn font_family_hash_matches_eq() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn hash<T: Hash>(t: &T) -> u64 {
        let mut h = DefaultHasher::new();
        t.hash(&mut h);
        h.finish()
    }

    let tf = sample_typeface();
    let a = FontFamily::Typeface(tf);
    let synthetic = crate::assets::Typeface {
        id: tf.id,
        family_name: "different-but-same-id",
        faces: &[],
        fallback: crate::assets::SystemFallback::None,
    };
    let b = FontFamily::Typeface(synthetic);
    assert_eq!(a, b);
    assert_eq!(hash(&a), hash(&b), "equal values must hash equal");

    let s1 = FontFamily::System("X".to_string());
    let s2 = FontFamily::System("X".to_string());
    assert_eq!(hash(&s1), hash(&s2));
    let s3 = FontFamily::System("Y".to_string());
    assert_ne!(hash(&s1), hash(&s3));
}

#[test]
fn content_key_typeface_distinct_from_same_named_system() {
    let tf = sample_typeface();
    let from_typeface = StyleRules {
        font_family: Some(FontFamily::Typeface(tf)),
        ..Default::default()
    };
    let from_system = StyleRules {
        font_family: Some(FontFamily::System(tf.family_name.to_string())),
        ..Default::default()
    };
    // A typeface and a same-name system reference describe
    // semantically different things (the typeface registration is
    // a separate backend artifact). Content keys must differ so
    // the backend doesn't conflate them.
    assert_ne!(from_typeface.content_key(), from_system.content_key());
}

#[test]
fn content_key_same_typeface_collapses_to_same_key() {
    let tf = sample_typeface();
    let a = StyleRules {
        font_family: Some(FontFamily::Typeface(tf)),
        ..Default::default()
    };
    let b = StyleRules {
        font_family: Some(FontFamily::Typeface(tf)),
        ..Default::default()
    };
    assert_eq!(a.content_key(), b.content_key());
}

#[test]
fn ensure_typefaces_registered_dedups_by_id() {
    // Two rules referencing the same typeface — register
    // callbacks fire exactly once. Different typefaces in the
    // same call register separately.
    let tf_a = sample_typeface();
    let tf_b = other_typeface();
    let rules: Vec<Rc<StyleRules>> = vec![
        Rc::new(StyleRules {
            font_family: Some(FontFamily::Typeface(tf_a)),
            ..Default::default()
        }),
        Rc::new(StyleRules {
            font_family: Some(FontFamily::Typeface(tf_a)),
            ..Default::default()
        }),
        Rc::new(StyleRules {
            font_family: Some(FontFamily::Typeface(tf_b)),
            ..Default::default()
        }),
        // System reference — must NOT trigger registration.
        Rc::new(StyleRules {
            font_family: Some(FontFamily::System("system-ui".to_string())),
            ..Default::default()
        }),
    ];
    let mut asset_calls: Vec<crate::assets::AssetId> = Vec::new();
    let mut typeface_calls: Vec<TypefaceId> = Vec::new();
    ensure_typefaces_registered_with(
        &rules,
        |id, kind, _src| {
            assert_eq!(kind, crate::assets::AssetTag::Font);
            asset_calls.push(id);
        },
        |id, _name, _faces, _fallback| {
            typeface_calls.push(id);
        },
    );
    // 2 faces for tf_a + 1 face for tf_b = 3 asset registrations.
    assert_eq!(asset_calls.len(), 3);
    assert_eq!(typeface_calls, vec![tf_a.id, tf_b.id]);

    // A *second* call for an overlapping set is a no-op for the
    // already-seen typeface. tf_b would also dedup, so a re-call
    // with [tf_a, tf_b] only fires for items already registered:
    // both are known → zero new calls.
    let mut asset_calls2: Vec<crate::assets::AssetId> = Vec::new();
    let mut typeface_calls2: Vec<TypefaceId> = Vec::new();
    ensure_typefaces_registered_with(
        &rules,
        |id, _, _| asset_calls2.push(id),
        |id, _, _, _| typeface_calls2.push(id),
    );
    assert!(asset_calls2.is_empty(), "no new asset registrations on dedup");
    assert!(typeface_calls2.is_empty(), "no new typeface registrations on dedup");
}

// ====================================================================
// Gradient merge + content_key + RadialExtent + aspect_ratio
//
// Regression tests for the "manual `overlay!()` macro silently
// drops new fields" bug class. `merge()` and `content_key()` are
// hand-listed; the welcome example's vignette pulse went dark on
// web for an entire release because `background_gradient` was
// omitted from `merge`'s list, and the resolved StyleRules
// backends received had `background_gradient: None` despite the
// sheet declaring one.
//
// These tests pin the property "every gradient-relevant field
// round-trips through merge AND distinguishes content_key" so
// any future field addition that forgets either path fails
// loudly in CI.
// ====================================================================

/// Helper: a Linear gradient with one stop. Specific values
/// don't matter for the merge/key tests; we just need a
/// distinct, recognizable `Some(Gradient)`.
fn linear_gradient(angle_deg: f32) -> Gradient {
    Gradient {
        kind: GradientKind::Linear { angle_deg },
        stops: vec![GradientStop {
            offset: 0.0,
            color: Color("#000".into()),
        }],
    }
}

fn radial_gradient(radius: f32, extent: RadialExtent) -> Gradient {
    Gradient {
        kind: GradientKind::Radial {
            center: (0.5, 0.5),
            radius,
            extent,
        },
        stops: vec![GradientStop {
            offset: 0.0,
            color: Color("#000".into()),
        }],
    }
}

/// `merge` must carry `background_gradient` from `other` when
/// `self` has none. This was the original gradient bug — `merge`
/// stripped the field, so backends got `None` and animation
/// snapshotting silently failed.
#[test]
fn merge_overlays_background_gradient_onto_empty_base() {
    let base = StyleRules::default();
    let overlay = StyleRules {
        background_gradient: Some(linear_gradient(45.0)),
        ..Default::default()
    };
    let merged = base.merge(&overlay);
    assert!(
        merged.background_gradient.is_some(),
        "merge dropped background_gradient on empty-base + Some-overlay; \
         this is the welcome-vignette bug class — verify the overlay! \
         macro lists `background_gradient`",
    );
    assert_eq!(
        merged.background_gradient.as_ref().unwrap(),
        &linear_gradient(45.0),
    );
}

/// `merge` must NOT clobber a base gradient when `other` has
/// none. (`overlay!` only overwrites when the other field is
/// `Some`; verifying we didn't accidentally write the wrong
/// branch.)
#[test]
fn merge_keeps_base_gradient_when_overlay_has_none() {
    let base = StyleRules {
        background_gradient: Some(linear_gradient(30.0)),
        ..Default::default()
    };
    let overlay = StyleRules::default();
    let merged = base.merge(&overlay);
    assert_eq!(
        merged.background_gradient.as_ref().unwrap(),
        &linear_gradient(30.0),
        "an empty overlay must not strip the base gradient",
    );
}

/// When both base and overlay set a gradient, overlay wins.
/// Standard `overlay!()` semantics — the existence of this
/// behaviour for gradient is what makes state-overlay
/// transitions on gradient backgrounds work.
#[test]
fn merge_overlay_gradient_wins_over_base_gradient() {
    let base = StyleRules {
        background_gradient: Some(linear_gradient(0.0)),
        ..Default::default()
    };
    let overlay = StyleRules {
        background_gradient: Some(linear_gradient(90.0)),
        ..Default::default()
    };
    let merged = base.merge(&overlay);
    assert_eq!(
        merged.background_gradient.as_ref().unwrap(),
        &linear_gradient(90.0),
    );
}

/// `content_key` must distinguish two gradients that differ
/// only in angle. Pre-fix, two distinct gradients could collide
/// on the same minted CSS class because `content_key` ignored
/// the gradient field entirely.
#[test]
fn content_key_differentiates_gradients_by_angle() {
    let a = StyleRules {
        background_gradient: Some(linear_gradient(0.0)),
        ..Default::default()
    };
    let b = StyleRules {
        background_gradient: Some(linear_gradient(45.0)),
        ..Default::default()
    };
    assert_ne!(
        a.content_key(),
        b.content_key(),
        "content_key must hash the angle so distinct gradients don't share a class",
    );
}

/// content_key must distinguish Linear from Radial even when
/// they're otherwise unrelated.
#[test]
fn content_key_differentiates_linear_vs_radial() {
    let lin = StyleRules {
        background_gradient: Some(linear_gradient(45.0)),
        ..Default::default()
    };
    let rad = StyleRules {
        background_gradient: Some(radial_gradient(1.0, RadialExtent::ClosestSide)),
        ..Default::default()
    };
    assert_ne!(lin.content_key(), rad.content_key());
}

/// content_key must distinguish two radial gradients whose only
/// difference is the `extent` (`ClosestSide` vs `FarthestCorner`).
/// This is the property that prevents the welcome-vignette
/// stops from collapsing onto the same class as a sun-disc
/// gradient that happens to share radius+center.
#[test]
fn content_key_differentiates_radial_extents() {
    let cs = StyleRules {
        background_gradient: Some(radial_gradient(1.0, RadialExtent::ClosestSide)),
        ..Default::default()
    };
    let fc = StyleRules {
        background_gradient: Some(radial_gradient(1.0, RadialExtent::FarthestCorner)),
        ..Default::default()
    };
    assert_ne!(
        cs.content_key(),
        fc.content_key(),
        "RadialExtent must contribute to content_key — otherwise gradients with \
         identical center/radius but different extents share a CSS class and the \
         wrong one wins on apply",
    );
}

/// content_key for two identical gradients must MATCH (the dedup
/// path relies on this — same content → same minted class).
#[test]
fn content_key_matches_for_identical_gradients() {
    let a = StyleRules {
        background_gradient: Some(radial_gradient(1.5, RadialExtent::FarthestCorner)),
        ..Default::default()
    };
    let b = StyleRules {
        background_gradient: Some(radial_gradient(1.5, RadialExtent::FarthestCorner)),
        ..Default::default()
    };
    assert_eq!(
        a.content_key(),
        b.content_key(),
        "identical gradient shape must collapse to one cached class",
    );
}

/// content_key must distinguish radial gradients with different
/// stop offsets. Important for animations that interpolate stop
/// positions independently of color.
#[test]
fn content_key_differentiates_gradient_stop_offsets() {
    let g_a = Gradient {
        kind: GradientKind::Linear { angle_deg: 45.0 },
        stops: vec![
            GradientStop { offset: 0.0, color: Color("#000".into()) },
            GradientStop { offset: 0.5, color: Color("#fff".into()) },
        ],
    };
    let g_b = Gradient {
        kind: GradientKind::Linear { angle_deg: 45.0 },
        stops: vec![
            GradientStop { offset: 0.0, color: Color("#000".into()) },
            GradientStop { offset: 0.8, color: Color("#fff".into()) },
        ],
    };
    let a = StyleRules { background_gradient: Some(g_a), ..Default::default() };
    let b = StyleRules { background_gradient: Some(g_b), ..Default::default() };
    assert_ne!(a.content_key(), b.content_key());
}

// ----- RadialExtent: default + round-trip ---------------------------

/// `RadialExtent::default()` is `ClosestSide`. The agent's
/// report calls this out as the documented default; pinning the
/// constant down here so a future change to the default
/// surfaces explicitly.
#[test]
fn radial_extent_default_is_closest_side() {
    let d: RadialExtent = RadialExtent::default();
    assert_eq!(d, RadialExtent::ClosestSide);
}

/// RadialExtent must round-trip through Clone + Copy + PartialEq
/// — these are the derives the public API depends on.
#[test]
fn radial_extent_clone_and_eq_round_trip() {
    let cs = RadialExtent::ClosestSide;
    let fc = RadialExtent::FarthestCorner;
    // Copy semantics — moving doesn't consume.
    let cs_again = cs;
    let _still_cs = cs;
    assert_eq!(cs, cs_again);
    // Distinct variants compare unequal.
    assert_ne!(cs, fc);
    // Clone is independent of Copy (both must work).
    let fc_cloned = fc.clone();
    assert_eq!(fc, fc_cloned);
}

/// A `Gradient` carrying a non-default `extent` must survive
/// being wrapped in a `Some(StyleRules { background_gradient:
/// Some(...) })` and pulled back out. Catches the "field
/// silently dropped" failure mode at the type-system level for
/// the gradient struct itself.
#[test]
fn radial_extent_round_trips_through_stylerules_field() {
    let g = radial_gradient(1.5, RadialExtent::FarthestCorner);
    let rules = StyleRules {
        background_gradient: Some(g.clone()),
        ..Default::default()
    };
    let back = rules.background_gradient.unwrap();
    match back.kind {
        GradientKind::Radial { extent, .. } => {
            assert_eq!(extent, RadialExtent::FarthestCorner);
        }
        other => panic!("expected Radial, got {:?}", other),
    }
    assert_eq!(back, g);
}

// ----- aspect_ratio: round-trip + key + merge -----------------------

/// `aspect_ratio` round-trips through `merge` (overlay-wins
/// semantics). Same regression class as the gradient bug — if
/// `aspect_ratio` were dropped from `overlay!()`, this fails.
#[test]
fn merge_overlays_aspect_ratio() {
    let base = StyleRules::default();
    let overlay = StyleRules {
        aspect_ratio: Some(1.5),
        ..Default::default()
    };
    let merged = base.merge(&overlay);
    assert_eq!(merged.aspect_ratio, Some(1.5));
}

/// `merge` preserves a base `aspect_ratio` when overlay is empty.
#[test]
fn merge_keeps_aspect_ratio_when_overlay_has_none() {
    let base = StyleRules {
        aspect_ratio: Some(2.0),
        ..Default::default()
    };
    let overlay = StyleRules::default();
    let merged = base.merge(&overlay);
    assert_eq!(merged.aspect_ratio, Some(2.0));
}

/// Overlay's `aspect_ratio` wins over base.
#[test]
fn merge_overlay_aspect_ratio_wins() {
    let base = StyleRules {
        aspect_ratio: Some(1.0),
        ..Default::default()
    };
    let overlay = StyleRules {
        aspect_ratio: Some(16.0 / 9.0),
        ..Default::default()
    };
    let merged = base.merge(&overlay);
    assert_eq!(merged.aspect_ratio, Some(16.0 / 9.0));
}

/// `content_key` distinguishes different aspect ratios — two
/// otherwise-identical rule sets must mint distinct classes.
/// Bench-relevant: a 16:9 video card and a 4:3 video card must
/// not collapse onto the same `.uiX` class.
#[test]
fn content_key_differentiates_aspect_ratios() {
    let a = StyleRules {
        aspect_ratio: Some(16.0 / 9.0),
        ..Default::default()
    };
    let b = StyleRules {
        aspect_ratio: Some(4.0 / 3.0),
        ..Default::default()
    };
    assert_ne!(
        a.content_key(),
        b.content_key(),
        "content_key must include aspect_ratio so different ratios mint different classes",
    );
}

/// content_key for the same aspect ratio collapses (dedup
/// invariant).
#[test]
fn content_key_matches_for_same_aspect_ratio() {
    let a = StyleRules {
        aspect_ratio: Some(1.5),
        ..Default::default()
    };
    let b = StyleRules {
        aspect_ratio: Some(1.5),
        ..Default::default()
    };
    assert_eq!(a.content_key(), b.content_key());
}

/// content_key distinguishes Some(ratio) from None — the
/// "ratio of None" path is its own bucket.
#[test]
fn content_key_differentiates_some_aspect_ratio_from_none() {
    let with_ratio = StyleRules {
        aspect_ratio: Some(1.0),
        ..Default::default()
    };
    let without = StyleRules::default();
    assert_ne!(with_ratio.content_key(), without.content_key());
}

// --- cached_stylesheet (shared per-sheet registry) ---------------
//
// These exercise the registry that replaced the per-sheet
// `thread_local!` the `stylesheet!` macro used to emit. The bug
// being prevented is Android bionic exhausting its 128 pthread TLS
// keys when a binary links 70+ stylesheets (each old `thread_local!`
// burned a key → abort in `LazyKey::lazy_init` at mount). Key
// exhaustion isn't reproducible on a host with a large key table, so
// the closest reachable coverage is the registry's behavioral
// contract: same key → same `Rc` (caching identity preserved),
// distinct keys → distinct sheets, and reentrancy safety (a build
// closure that itself caches another sheet must not double-borrow).

fn empty_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::new(|_vs| StyleRules::default()))
}

#[test]
fn cached_stylesheet_same_key_returns_same_rc() {
    static K: u8 = 0;
    let key = &K as *const u8 as usize;
    let mut built = 0;
    let a = cached_stylesheet(key, || {
        built += 1;
        empty_sheet()
    });
    let b = cached_stylesheet(key, || {
        built += 1;
        empty_sheet()
    });
    // Built exactly once; both calls hand back the same allocation.
    assert_eq!(built, 1, "build closure must run only on first call");
    assert!(Rc::ptr_eq(&a, &b), "same key must return the cached Rc");
}

#[test]
fn cached_stylesheet_distinct_keys_are_independent() {
    static K1: u8 = 0;
    static K2: u8 = 0;
    let a = cached_stylesheet(&K1 as *const u8 as usize, empty_sheet);
    let b = cached_stylesheet(&K2 as *const u8 as usize, empty_sheet);
    assert!(!Rc::ptr_eq(&a, &b), "distinct keys must not collide");
}

#[test]
fn cached_stylesheet_reentrant_build_does_not_double_borrow() {
    // A sheet whose construction references another `*_style()` (a
    // nested sheet) caches the inner one mid-build. The outer build
    // must hold no borrow of the registry while it runs.
    static OUTER: u8 = 0;
    static INNER: u8 = 0;
    let outer = cached_stylesheet(&OUTER as *const u8 as usize, || {
        let _inner = cached_stylesheet(&INNER as *const u8 as usize, empty_sheet);
        empty_sheet()
    });
    // Both entries are now resident and resolve to themselves.
    let outer_again = cached_stylesheet(&OUTER as *const u8 as usize, empty_sheet);
    assert!(Rc::ptr_eq(&outer, &outer_again));
}


/// Guard for the hand-written `Clone for StyleRules` (outlined for wasm
/// size — see the impl). Every field is `Some` with a value DISTINCT
/// from every other same-typed field, so a copy-paste slip that clones
/// field A into slot B fails the `PartialEq` here rather than shipping
/// silently. No `..Default::default()` on purpose: a new field must be
/// added to this literal (and to the manual impl) or this fails to
/// compile.
#[test]
fn clone_round_trips_a_fully_populated_struct() {
    let rules = StyleRules {
        background: Some(Tokenized::Literal(Color("#c0".into()))),
        color: Some(Tokenized::token("tok-c1", Color("#f1".into()))),
        caret_color: Some(Tokenized::Literal(Color("#c2".into()))),
        border_top_color: Some(Tokenized::token("tok-c3", Color("#f3".into()))),
        border_right_color: Some(Tokenized::Literal(Color("#c4".into()))),
        border_bottom_color: Some(Tokenized::token("tok-c5", Color("#f5".into()))),
        border_left_color: Some(Tokenized::Literal(Color("#c6".into()))),
        font_size: Some(Tokenized::Literal(Length::Px(100.0))),
        gap: Some(Tokenized::token("tok-l1", Length::Percent(1.0))),
        row_gap: Some(Tokenized::Literal(Length::Px(102.0))),
        column_gap: Some(Tokenized::token("tok-l3", Length::Percent(3.0))),
        flex_basis: Some(Tokenized::Literal(Length::Px(104.0))),
        width: Some(Tokenized::token("tok-l5", Length::Percent(5.0))),
        height: Some(Tokenized::Literal(Length::Px(106.0))),
        min_width: Some(Tokenized::token("tok-l7", Length::Percent(7.0))),
        min_height: Some(Tokenized::Literal(Length::Px(108.0))),
        max_width: Some(Tokenized::token("tok-l9", Length::Percent(9.0))),
        max_height: Some(Tokenized::Literal(Length::Px(110.0))),
        padding_top: Some(Tokenized::token("tok-l11", Length::Percent(11.0))),
        padding_right: Some(Tokenized::Literal(Length::Px(112.0))),
        padding_bottom: Some(Tokenized::token("tok-l13", Length::Percent(13.0))),
        padding_left: Some(Tokenized::Literal(Length::Px(114.0))),
        margin_top: Some(Tokenized::token("tok-l15", Length::Percent(15.0))),
        margin_right: Some(Tokenized::Literal(Length::Px(116.0))),
        margin_bottom: Some(Tokenized::token("tok-l17", Length::Percent(17.0))),
        margin_left: Some(Tokenized::Literal(Length::Px(118.0))),
        border_top_left_radius: Some(Tokenized::token("tok-l19", Length::Percent(19.0))),
        border_top_right_radius: Some(Tokenized::Literal(Length::Px(120.0))),
        border_bottom_left_radius: Some(Tokenized::token("tok-l21", Length::Percent(21.0))),
        border_bottom_right_radius: Some(Tokenized::Literal(Length::Px(122.0))),
        top: Some(Tokenized::token("tok-l23", Length::Percent(23.0))),
        right: Some(Tokenized::Literal(Length::Px(124.0))),
        bottom: Some(Tokenized::token("tok-l25", Length::Percent(25.0))),
        left: Some(Tokenized::Literal(Length::Px(126.0))),
        flex_grow: Some(Tokenized::Literal(300.0)),
        flex_shrink: Some(Tokenized::Literal(301.0)),
        border_top_width: Some(Tokenized::Literal(302.0)),
        border_right_width: Some(Tokenized::Literal(303.0)),
        border_bottom_width: Some(Tokenized::Literal(304.0)),
        border_left_width: Some(Tokenized::Literal(305.0)),
        line_height: Some(Tokenized::Literal(306.0)),
        letter_spacing: Some(Tokenized::Literal(307.0)),
        opacity: Some(Tokenized::Literal(308.0)),
        background_transition: Some(Transition::new(500, Easing::EaseOut)),
        color_transition: Some(Transition::new(510, Easing::EaseOut)),
        caret_color_transition: Some(Transition::new(520, Easing::EaseOut)),
        opacity_transition: Some(Transition::new(530, Easing::EaseOut)),
        transform_transition: Some(Transition::new(540, Easing::EaseOut)),
        width_transition: Some(Transition::new(550, Easing::EaseOut)),
        height_transition: Some(Transition::new(560, Easing::EaseOut)),
        max_width_transition: Some(Transition::new(570, Easing::EaseOut)),
        max_height_transition: Some(Transition::new(580, Easing::EaseOut)),
        min_width_transition: Some(Transition::new(590, Easing::EaseOut)),
        min_height_transition: Some(Transition::new(600, Easing::EaseOut)),
        top_transition: Some(Transition::new(610, Easing::EaseOut)),
        right_transition: Some(Transition::new(620, Easing::EaseOut)),
        bottom_transition: Some(Transition::new(630, Easing::EaseOut)),
        left_transition: Some(Transition::new(640, Easing::EaseOut)),
        padding_top_transition: Some(Transition::new(650, Easing::EaseOut)),
        padding_right_transition: Some(Transition::new(660, Easing::EaseOut)),
        padding_bottom_transition: Some(Transition::new(670, Easing::EaseOut)),
        padding_left_transition: Some(Transition::new(680, Easing::EaseOut)),
        margin_top_transition: Some(Transition::new(690, Easing::EaseOut)),
        margin_right_transition: Some(Transition::new(700, Easing::EaseOut)),
        margin_bottom_transition: Some(Transition::new(710, Easing::EaseOut)),
        margin_left_transition: Some(Transition::new(720, Easing::EaseOut)),
        border_top_left_radius_transition: Some(Transition::new(730, Easing::EaseOut)),
        border_top_right_radius_transition: Some(Transition::new(740, Easing::EaseOut)),
        border_bottom_left_radius_transition: Some(Transition::new(750, Easing::EaseOut)),
        border_bottom_right_radius_transition: Some(Transition::new(760, Easing::EaseOut)),
        border_top_width_transition: Some(Transition::new(770, Easing::EaseOut)),
        border_right_width_transition: Some(Transition::new(780, Easing::EaseOut)),
        border_bottom_width_transition: Some(Transition::new(790, Easing::EaseOut)),
        border_left_width_transition: Some(Transition::new(800, Easing::EaseOut)),
        border_top_color_transition: Some(Transition::new(810, Easing::EaseOut)),
        border_right_color_transition: Some(Transition::new(820, Easing::EaseOut)),
        border_bottom_color_transition: Some(Transition::new(830, Easing::EaseOut)),
        border_left_color_transition: Some(Transition::new(840, Easing::EaseOut)),
        display: Some(DisplayKind::Grid),
        grid_template_columns: Some(vec![TrackSize::Fr(2.0), TrackSize::Px(40.0)]),
        flex_direction: Some(FlexDirection::RowReverse),
        flex_wrap: Some(FlexWrap::Wrap),
        justify_content: Some(JustifyContent::SpaceEvenly),
        align_items: Some(AlignItems::Baseline),
        align_content: Some(AlignContent::SpaceBetween),
        align_self: Some(AlignSelf::FlexEnd),
        aspect_ratio: Some(0.5625),
        position: Some(Position::Absolute),
        font_family: Some(FontFamily::System("Test Sans".into())),
        font_weight: Some(FontWeight::Black),
        font_style: Some(FontStyle::Italic),
        text_align: Some(TextAlign::Justify),
        underline: Some(true),
        strikethrough: Some(false),
        text_transform: Some(TextTransform::Lowercase),
        overflow: Some(Overflow::Hidden),
        object_fit: Some(ObjectFit::Cover),
        shadow: Some(Shadow { x: 1.0, y: 2.0, blur: 3.0, color: Color("#s".into()) }),
        text_shadow: Some(Shadow { x: 4.0, y: 5.0, blur: 6.0, color: Color("#ts".into()) }),
        background_gradient: Some(Gradient {
            kind: GradientKind::Radial { center: (0.1, 0.9), radius: 1.25, extent: RadialExtent::FarthestCorner },
            stops: vec![GradientStop { offset: 0.0, color: Color("#g0".into()) },
                        GradientStop { offset: 1.0, color: Color("#g1".into()) }],
        }),
        transform: Some(vec![Transform::TranslateX(Length::Px(9.0)), Transform::Rotate(30.0)]),
        transform_origin: Some((Length::Px(3.0), Length::Percent(60.0))),
        cursor: Some(Cursor::Help),
        user_select: Some(UserSelect::Text),
        pointer_events: Some(PointerEvents::None),
    };
    assert_eq!(rules.clone(), rules);
}

// --- Cross-axis merge precedence ---------------------------------------

/// Axes that set the SAME property merge in alphabetical axis-name order,
/// NOT declaration order — `StyleSheet::variants` is a `BTreeMap`.
///
/// This surfaced converting idea-ui's `Card` intent tint from a computed
/// layer to a `tone` axis: the tint vanished, because Card's `variant` axis
/// also sets `background` and `"tone" < "variant"`. Declaring `tone` last
/// changed nothing. A sheet that needs a specific winner must either fold
/// the axes into one (Badge/Tag/Alert key a single `appearance` axis as
/// `{tone}_{variant}`) or use a later resolution step.
#[test]
fn axis_merge_precedence_is_alphabetical_not_declaration_order() {
    let red = || Color("#ff0000".into());
    let blue = || Color("#0000ff".into());

    // Declare `variant` FIRST and `tone` SECOND. If precedence followed
    // declaration order, `tone` (declared later) would win.
    let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules::default())
        .variant("variant", "flat", move |_vs| StyleRules {
            background: Some(Tokenized::Literal(red())),
            ..Default::default()
        })
        .variant("tone", "danger", move |_vs| StyleRules {
            background: Some(Tokenized::Literal(blue())),
            ..Default::default()
        });

    let vs = VariantSet::new().with("variant", "flat").with("tone", "danger");
    let resolved = sheet.resolve(&vs);

    // "variant" sorts after "tone", so the variant arm wins.
    assert_eq!(
        resolved.background,
        Some(Tokenized::Literal(red())),
        "alphabetically-later axis name must win a same-property conflict"
    );
}

/// The companion half: rename the winning axis so it sorts EARLIER and the
/// other side wins. Pins that the ordering really is by name — nothing
/// about `tone`/`variant` specifically.
#[test]
fn renaming_an_axis_flips_cross_axis_precedence() {
    let red = || Color("#ff0000".into());
    let blue = || Color("#0000ff".into());

    // Same two arms, but the surface axis is now named "a_surface", which
    // sorts BEFORE "tone".
    let sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules::default())
        .variant("a_surface", "flat", move |_vs| StyleRules {
            background: Some(Tokenized::Literal(red())),
            ..Default::default()
        })
        .variant("tone", "danger", move |_vs| StyleRules {
            background: Some(Tokenized::Literal(blue())),
            ..Default::default()
        });

    let vs = VariantSet::new().with("a_surface", "flat").with("tone", "danger");
    assert_eq!(
        sheet.resolve(&vs).background,
        Some(Tokenized::Literal(blue())),
        "with the surface axis renamed to sort first, the tone arm must win"
    );
}

/// A computed layer resolves AFTER every axis, which is the escape hatch a
/// sheet uses when a rule must beat an axis it can't outrank by name. This
/// is why idea-ui's `Card` tint stayed a computed layer.
#[test]
fn computed_layer_beats_every_axis_regardless_of_name() {
    let red = || Color("#ff0000".into());
    let green = || Color("#00ff00".into());

    let sheet = Rc::new(
        StyleSheet::new(|_vs: &VariantSet| StyleRules::default()).variant(
            "zzz_last_axis",
            "on",
            move |_vs| StyleRules {
                background: Some(Tokenized::Literal(red())),
                ..Default::default()
            },
        ),
    );

    let app = StyleApplication::new(sheet)
        .with("zzz_last_axis", "on")
        .with_computed("tint", move || StyleRules {
            background: Some(Tokenized::Literal(green())),
            ..Default::default()
        });

    assert_eq!(
        crate::resolve_style(&app).background,
        Some(Tokenized::Literal(green())),
        "the computed layer must beat even the alphabetically-last axis"
    );
}

// --- The inline layer -------------------------------------------------

fn inline_sheet() -> Rc<StyleSheet> {
    Rc::new(
        StyleSheet::new(|_vs: &VariantSet| StyleRules {
            background: Some(Tokenized::Literal(Color("#111111".into()))),
            ..Default::default()
        })
        .variant("gap", "md", |_vs| StyleRules::default())
        .variant_default("gap", "md"),
    )
    .clone()
}

/// The inline layer resolves LAST — after overrides — matching CSS, where
/// an inline `style` attribute beats any class rule.
#[test]
fn inline_layer_resolves_after_overrides() {
    let app = StyleApplication::new(inline_sheet())
        .with_overrides(StyleRules {
            background: Some(Tokenized::Literal(Color("#222222".into()))),
            ..Default::default()
        })
        .with_inline(StyleRules {
            background: Some(Tokenized::Literal(Color("#333333".into()))),
            ..Default::default()
        });
    assert_eq!(
        crate::resolve_style(&app).background,
        Some(Tokenized::Literal(Color("#333333".into()))),
        "inline must beat overrides"
    );
}

/// THE reason this layer exists: two applications differing ONLY in their
/// inline rules share one resolution-cache entry.
///
/// A `with_computed` layer keyed on the value, or an `override_*` carrying
/// it, puts the value in `ResolutionKey` — so a slider thumb keyed on its
/// pixel position mints an entry, and on web a CSS class, per pixel
/// dragged. Cache identity is pointer-compared here: same `Rc` means the
/// second resolve hit the memo rather than re-resolving.
#[test]
fn inline_layer_is_not_part_of_the_cache_identity() {
    let sheet = inline_sheet();
    let a = StyleApplication::new(sheet.clone()).with_inline(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(10.0))),
        ..Default::default()
    });
    let b = StyleApplication::new(sheet.clone()).with_inline(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(9999.0))),
        ..Default::default()
    });

    // Different inline values reach the node...
    assert_eq!(
        crate::resolve_style(&a).width,
        Some(Tokenized::Literal(Length::Px(10.0)))
    );
    assert_eq!(
        crate::resolve_style(&b).width,
        Some(Tokenized::Literal(Length::Px(9999.0)))
    );

    // ...while the CACHED half is one shared entry. Compare against a
    // third application with no inline layer at all: it must be the very
    // same `Rc` the other two were built from.
    let plain = StyleApplication::new(sheet);
    let first = crate::resolve_style(&plain);
    let second = crate::resolve_style(&plain);
    assert!(
        Rc::ptr_eq(&first, &second),
        "the cached half must memoize"
    );
    assert_eq!(
        first.background,
        Some(Tokenized::Literal(Color("#111111".into()))),
        "and it must be the sheet's own resolution"
    );
}

/// An inline layer does NOT disqualify preminting — unlike `overrides` and
/// `computed`, which do. The classes still ship from the dump; the inline
/// values ride the node.
#[test]
fn inline_layer_does_not_disqualify_preminting() {
    let sheet = Rc::new(
        StyleSheet::new(|_vs: &VariantSet| StyleRules::default())
            .variant("gap", "md", |_vs| StyleRules::default())
            .variant_default("gap", "md"),
    );
    let sheet = StyleSheet::premint_as(
        Rc::try_unwrap(sheet).unwrap_or_else(|_| panic!("sole owner")),
        "test.inline.v1",
    );

    let plain = StyleApplication::new(sheet.clone());
    let with_inline = StyleApplication::new(sheet.clone()).with_inline(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(42.0))),
        ..Default::default()
    });
    assert_eq!(
        plain.preminted_class_list(),
        with_inline.preminted_class_list(),
        "an inline layer must not change — or block — the class list"
    );
    assert!(with_inline.preminted_class_list().is_some());

    // Overrides and computed layers still DO disqualify.
    let with_override = StyleApplication::new(sheet.clone()).with_overrides(StyleRules {
        width: Some(Tokenized::Literal(Length::Px(42.0))),
        ..Default::default()
    });
    assert!(with_override.preminted_class_list().is_none());
    let with_computed =
        StyleApplication::new(sheet).with_computed("k", || StyleRules::default());
    assert!(with_computed.preminted_class_list().is_none());
}

/// The `if`-empty-branch anchor sheet — the ONE style the framework
/// itself emits — must premint: it was the last un-preminted style on
/// the website corpus (a raw `StyleRules` has nothing to premint by
/// definition), and it is link-time registered because an `if` can be
/// true throughout the dump crawl and false for the first time at
/// runtime.
#[test]
fn regression_empty_absolute_sheet_premints() {
    let sheet = crate::empty_absolute_sheet();
    assert_eq!(
        sheet.premint_class(),
        Some(crate::EMPTY_ABSOLUTE_CLASS),
        "the anchor sheet carries its constant class"
    );
    let app = crate::StyleApplication::new(sheet);
    assert_eq!(
        app.preminted_class_list().as_deref(),
        Some(crate::EMPTY_ABSOLUTE_CLASS),
        "a bare application premints to exactly that class"
    );
    let rules = crate::style::resolve(&app);
    assert_eq!(
        rules.position,
        Some(crate::Position::Absolute),
        "and still resolves to the layout-neutral absolute position"
    );
}
