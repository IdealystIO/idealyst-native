// Exposes the smoke staticlib's C-exported symbols to Swift — same
// contract as the CLI-generated wrapper's bridging header, minus the
// deep-link hook (no navigator in the smoke tree).

void ios_main(void *root_view);
void ios_teardown(void);
