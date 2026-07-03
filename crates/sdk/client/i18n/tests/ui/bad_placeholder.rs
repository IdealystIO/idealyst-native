fn main() {}

mod t {
    i18n::i18n! {
        locales: { En = "en" (default) }

        // `{nmae}` is a typo for the `name` argument -> compile error.
        greeting(name) {
            En: "Hello, {nmae}",
        }
    }
}
