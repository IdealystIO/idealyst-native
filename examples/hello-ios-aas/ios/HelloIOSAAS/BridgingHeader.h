// Bridging header — exposes the Rust C-exported symbols to Swift.
//
// `app_id_utf8` must point to a NUL-terminated UTF-8 string matching
// the dev-server's mDNS TXT record's `app_id` field. The Rust side
// browses Bonjour for `_idealyst-dev._tcp.` and connects to the
// first server whose `app_id` matches.

void ios_main(void *root_view, const char *app_id_utf8);
void ios_teardown(void);
