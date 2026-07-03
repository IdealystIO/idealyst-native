fn main() {}

mod t {
    i18n::i18n! {
        // No locale marked `(default)` -> compile error.
        locales: { En = "en", Fr = "fr" }

        hello {
            En: "Hi",
            Fr: "Salut",
        }
    }
}
