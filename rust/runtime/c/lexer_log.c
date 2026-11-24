// The one piece of the lexer that must stay C: `TSLexer.log` is a variadic function, and stable
// Rust cannot define one. It formats the message and hands the buffer to the Rust lexer, which
// forwards it to the user's logger. Remove when `c_variadic` is stable (see rust/DECISIONS.md).
#include <stdarg.h>
#include <stdio.h>
#include "parser.h"

void ts_lexer_log_emit(const TSLexer *self, const char *buffer);

void ts_lexer__log(const TSLexer *self, const char *fmt, ...) {
  char buffer[TREE_SITTER_SERIALIZATION_BUFFER_SIZE];
  va_list args;
  va_start(args, fmt);
  vsnprintf(buffer, TREE_SITTER_SERIALIZATION_BUFFER_SIZE, fmt, args);
  va_end(args);
  ts_lexer_log_emit(self, buffer);
}
