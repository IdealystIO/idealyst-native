fn main() {}

mod t {
    i18n::i18n! {
        locales: { En = "en" (default), Fr = "fr" }

        // `Fr` is bundled but this message omits it -> compile error.
        greeting(name) {
            En: "Hello, {name}",
        }
    }
}
