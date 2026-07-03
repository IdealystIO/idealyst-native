fn main() {}

mod t {
    i18n::i18n! {
        locales: { En = "en" (default), Ja = "ja" (lazy) }

        // `Ja` is lazy: its strings come from a fetched pack, so an inline
        // translation is a compile error.
        hello {
            En: "Hi",
            Ja: "やあ",
        }
    }
}
