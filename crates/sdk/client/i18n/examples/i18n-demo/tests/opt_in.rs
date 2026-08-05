//! Reproduces the runtime-server host path: `app()` installs the opt-in
//! loader, switching to the lazy locale loads its pack, and the message
//! resolves to the localized string. Runs native (host), like dev mode.

#[test]
fn opt_in_locale_loads_from_disk_on_native() {
    // The app body creates signals, and on runtime v2 `signal()` resolves
    // the AMBIENT world — calling `app()` at top level aborts with
    // "signal()/effect() called outside World::enter". Everything that
    // touches the reactive graph (the build, the locale switch, and the
    // message read, which subscribes) has to happen inside the same world.
    let world = runtime_world::World::new();
    world.enter(|| {
        // Building the app installs the (native = disk) pack loader.
        let _tree = i18n_demo::app();

        // Before the pack loads, the lazy locale falls back to the default.
        i18n_demo::t::set_locale(i18n_demo::t::Locale::Ja);

        // The native loader reads `locales/ja.json` synchronously and
        // installs the pack, so the message resolves to Japanese.
        assert_eq!(i18n_demo::t::greeting("Ada").get(), "こんにちは、Ada");
    });
}
