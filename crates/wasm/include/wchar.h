#ifndef CONTINUITY_WASM_WCHAR_H
#define CONTINUITY_WASM_WCHAR_H

/* tree-sitter-language supplies the locale-free wide predicates. */
#include <wctype.h>

static inline wint_t towlower(wint_t character) {
    if (character >= 'A' && character <= 'Z') {
        return character + ('a' - 'A');
    }
    return character;
}

/* The upstream minimal WASM headers omit two functions used by the Markdown
 * external scanner. CommonMark restricts both call sites to ASCII data. */
static inline int isdigit(int character) {
    return character >= '0' && character <= '9';
}

static inline int strcmp(const char *left, const char *right) {
    while (*left != '\0' && *left == *right) {
        left++;
        right++;
    }
    return (unsigned char)*left - (unsigned char)*right;
}

#endif
