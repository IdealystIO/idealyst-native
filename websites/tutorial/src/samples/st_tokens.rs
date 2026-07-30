use runtime_core::{install_tokens, update_tokens, Color, Length, TokenEntry, TokenValue};

fn install_once() {
    install_tokens(&[
        TokenEntry { name: "color-accent", value: TokenValue::Color(Color("#5b6cff".into())) },
        TokenEntry { name: "spacing-md", value: TokenValue::Length(Length::Px(12.0)) },
    ]);
}

fn swap_to_dark() {
    // Staged like any other write. At the flush, only nodes that resolved a
    // changed token re-apply; everything else is untouched.
    update_tokens(&[TokenEntry {
        name: "color-accent",
        value: TokenValue::Color(Color("#22d3ee".into())),
    }]);
}
