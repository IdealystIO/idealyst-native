// Bridging header — exposes the Rust C-exported symbols to Swift.
// `url_utf8` may be NULL to use the default `ws://127.0.0.1:9001`.

void ios_main(void *root_view, const char *url_utf8);
void ios_teardown(void);
