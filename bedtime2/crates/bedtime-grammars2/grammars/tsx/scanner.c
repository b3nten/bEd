#include "common/scanner.h"

void *bed2_tree_sitter_tsx_external_scanner_create() { return NULL; }

void bed2_tree_sitter_tsx_external_scanner_destroy(void *payload) {}

unsigned bed2_tree_sitter_tsx_external_scanner_serialize(void *payload, char *buffer) { return 0; }

void bed2_tree_sitter_tsx_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {}

bool bed2_tree_sitter_tsx_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
    return external_scanner_scan(payload, lexer, valid_symbols);
}
