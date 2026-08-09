#include "continuity_engine.h"
#include "continuity_parity_fixture.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(call) do { if ((call) != CONTINUITY_ENGINE_OK) fail(#call); } while (0)

typedef struct CallbackProbe {
    ContinuityEngineHandle *handle;
    uint64_t revision;
    ContinuityEngineStatus reentrant_status;
} CallbackProbe;

static void fail(const char *operation) {
    size_t required = 0;
    continuity_engine_last_error_utf8(NULL, 0, &required);
    uint8_t *message = required == 0 ? NULL : (uint8_t *)malloc(required + 1);
    if (message != NULL) {
        continuity_engine_last_error_utf8(message, required, &required);
        message[required] = 0;
    }
    fprintf(stderr, "%s failed: %s\n", operation, message == NULL ? "" : (char *)message);
    free(message);
    exit(1);
}

static void on_change(void *user_data, uint64_t revision) {
    CallbackProbe *probe = (CallbackProbe *)user_data;
    uint64_t ignored = 0;
    probe->revision = revision;
    probe->reentrant_status = continuity_engine_revision(probe->handle, &ignored);
}

static ContinuityEngineHandle *create(const char *text) {
    ContinuityEngineHandle *handle = NULL;
    CHECK(continuity_engine_create_utf8(
        CONTINUITY_ENGINE_ABI_MAJOR,
        (const uint8_t *)text,
        strlen(text),
        0,
        &handle));
    return handle;
}

static void expect_text(ContinuityEngineHandle *handle, const char *expected) {
    ContinuityEngineString snapshot = {0};
    CHECK(continuity_engine_snapshot_utf8(handle, &snapshot));
    if (snapshot.len != strlen(expected) || memcmp(snapshot.data, expected, snapshot.len) != 0) {
        fprintf(stderr, "unexpected snapshot\n");
        exit(1);
    }
    CHECK(continuity_engine_string_free(snapshot));
}

static void insert(ContinuityEngineHandle *handle, const char *text, int64_t timestamp) {
    CHECK(continuity_engine_insert_utf8(
        handle,
        (const uint8_t *)text,
        strlen(text),
        timestamp));
}

static void multi_cursor_case(void) {
    ContinuityEngineHandle *handle = create(CONTINUITY_FIXTURE_MULTI_INITIAL);
    ContinuityEnginePosition carets[] = {
        {CONTINUITY_FIXTURE_MULTI_CARET0_LINE, CONTINUITY_FIXTURE_MULTI_CARET0_BYTE},
        {CONTINUITY_FIXTURE_MULTI_CARET1_LINE, CONTINUITY_FIXTURE_MULTI_CARET1_BYTE}};
    CallbackProbe probe = {handle, 0, CONTINUITY_ENGINE_OK};
    CHECK(continuity_engine_set_carets(handle, carets, 2));
    CHECK(continuity_engine_set_change_callback(handle, on_change, &probe));
    insert(handle, CONTINUITY_FIXTURE_MULTI_INSERT, 1000);
    expect_text(handle, CONTINUITY_FIXTURE_MULTI_EXPECTED);
    if (probe.revision != CONTINUITY_FIXTURE_MULTI_REVISION ||
        probe.reentrant_status != CONTINUITY_ENGINE_REENTRANT_CALL) {
        fprintf(stderr, "callback contract failed\n");
        exit(1);
    }

    ContinuityEnginePosition *result_carets = NULL;
    size_t caret_count = 0;
    CHECK(continuity_engine_carets(handle, &result_carets, &caret_count));
    if (caret_count != 2 ||
        result_carets[0].byte_in_line != CONTINUITY_FIXTURE_MULTI_EXPECTED_CARET0_BYTE ||
        result_carets[1].byte_in_line != CONTINUITY_FIXTURE_MULTI_EXPECTED_CARET1_BYTE) {
        fprintf(stderr, "multi-cursor contract failed\n");
        exit(1);
    }
    CHECK(continuity_engine_carets_free(result_carets, caret_count));

    ContinuityEngineDelta *deltas = NULL;
    size_t delta_count = 0;
    CHECK(continuity_engine_deltas_since(handle, 0, &deltas, &delta_count));
    if (delta_count != 2 || deltas[0].at != CONTINUITY_FIXTURE_MULTI_DELTA0_AT ||
        deltas[1].at != CONTINUITY_FIXTURE_MULTI_DELTA1_AT) {
        fprintf(stderr, "delta contract failed\n");
        exit(1);
    }
    CHECK(continuity_engine_deltas_free(deltas, delta_count));
    CHECK(continuity_engine_destroy(handle));
}

static void unicode_delete_case(void) {
    ContinuityEngineHandle *handle = create(CONTINUITY_FIXTURE_DELETE_INITIAL);
    ContinuityEnginePosition caret = {
        CONTINUITY_FIXTURE_DELETE_CARET_LINE,
        CONTINUITY_FIXTURE_DELETE_CARET_BYTE};
    CHECK(continuity_engine_set_carets(handle, &caret, 1));
    CHECK(continuity_engine_delete_backward(handle, 2000));
    expect_text(handle, CONTINUITY_FIXTURE_DELETE_EXPECTED);
    ContinuityEnginePosition *result_carets = NULL;
    size_t caret_count = 0;
    CHECK(continuity_engine_carets(handle, &result_carets, &caret_count));
    if (caret_count != 1 ||
        result_carets[0].line != CONTINUITY_FIXTURE_DELETE_EXPECTED_CARET_LINE ||
        result_carets[0].byte_in_line != CONTINUITY_FIXTURE_DELETE_EXPECTED_CARET_BYTE) {
        fprintf(stderr, "Unicode deletion caret contract failed\n");
        exit(1);
    }
    CHECK(continuity_engine_carets_free(result_carets, caret_count));
    CHECK(continuity_engine_destroy(handle));
}

static void typing_undo_case(void) {
    ContinuityEngineHandle *handle = create("");
    insert(handle, CONTINUITY_FIXTURE_TYPING0, 3000);
    insert(handle, CONTINUITY_FIXTURE_TYPING1, 3100);
    insert(handle, CONTINUITY_FIXTURE_TYPING2, 3200);
    expect_text(handle, CONTINUITY_FIXTURE_TYPING_EXPECTED);
    CHECK(continuity_engine_undo(handle, 3400));
    expect_text(handle, CONTINUITY_FIXTURE_TYPING_AFTER_UNDO);
    CHECK(continuity_engine_redo(handle, 3500));
    expect_text(handle, CONTINUITY_FIXTURE_TYPING_AFTER_REDO);
    CHECK(continuity_engine_destroy(handle));
}

static void undo_branch_case(void) {
    ContinuityEngineHandle *handle = create("");
    insert(handle, CONTINUITY_FIXTURE_BRANCH_PREFIX, 3000);
    insert(handle, CONTINUITY_FIXTURE_BRANCH_OLD, 3001);
    CHECK(continuity_engine_undo(handle, 3002));
    insert(handle, CONTINUITY_FIXTURE_BRANCH_NEW, 3003);
    expect_text(handle, CONTINUITY_FIXTURE_BRANCH_REPLACEMENT);
    CHECK(continuity_engine_undo(handle, 3004));
    CHECK(continuity_engine_redo_alternate(handle, 3005));
    expect_text(handle, CONTINUITY_FIXTURE_BRANCH_ALTERNATE);
    CHECK(continuity_engine_destroy(handle));
}

int main(void) {
    ContinuityEngineCapabilities capabilities = {0};
    CHECK(continuity_engine_capabilities(&capabilities));
    if (capabilities.abi_major != CONTINUITY_ENGINE_ABI_MAJOR ||
        capabilities.sdk_major != 0 || capabilities.sdk_minor != 1 || capabilities.sdk_patch != 0 ||
        (capabilities.flags & CONTINUITY_ENGINE_CAP_UTF16) == 0) {
        fprintf(stderr, "capability negotiation failed\n");
        return 1;
    }
    multi_cursor_case();
    unicode_delete_case();
    typing_undo_case();
    undo_branch_case();
    printf("CONTINUITY_C_PARITY {\"abiMajor\":1,\"status\":\"passed\"}\n");
    return 0;
}
