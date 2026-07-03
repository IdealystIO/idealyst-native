fn main() {}

mod t {
    i18n::i18n! {
        locales: { En = "en" (default) }

        // `name` is declared but never used in the default translation.
        greeting(name) {
            En: "Hello there",
        }
    }
}
