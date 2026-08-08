#ifndef CONTINUITY_ENGINE_H
#define CONTINUITY_ENGINE_H

#include <stddef.h>
#include <stdint.h>

#ifdef _WIN32
#define CONTINUITY_ENGINE_API __declspec(dllimport)
#else
#define CONTINUITY_ENGINE_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

#define CONTINUITY_ENGINE_ABI_MAJOR 1
#define CONTINUITY_ENGINE_ABI_MINOR 0
#define CONTINUITY_ENGINE_SDK_VERSION "0.2.25"
#define CONTINUITY_ENGINE_CAP_UTF16 (UINT64_C(1) << 0)
#define CONTINUITY_ENGINE_CAP_CALLBACK (UINT64_C(1) << 1)
#define CONTINUITY_ENGINE_CAP_MULTI_CURSOR (UINT64_C(1) << 2)
#define CONTINUITY_ENGINE_CAP_BRANCHING_UNDO (UINT64_C(1) << 3)

typedef enum ContinuityEngineStatus {
    CONTINUITY_ENGINE_OK = 0,
    CONTINUITY_ENGINE_NULL_POINTER = 1,
    CONTINUITY_ENGINE_INVALID_UTF8 = 2,
    CONTINUITY_ENGINE_INVALID_UTF16 = 3,
    CONTINUITY_ENGINE_INVALID_POSITION = 4,
    CONTINUITY_ENGINE_WRONG_THREAD = 5,
    CONTINUITY_ENGINE_REENTRANT_CALL = 6,
    CONTINUITY_ENGINE_UNSUPPORTED_ABI = 7,
    CONTINUITY_ENGINE_ERROR = 8,
    CONTINUITY_ENGINE_PANIC = 9
} ContinuityEngineStatus;

typedef struct ContinuityEngineHandle ContinuityEngineHandle;
typedef struct ContinuityEngineCapabilities {
    uint32_t struct_size;
    uint16_t abi_major;
    uint16_t abi_minor;
    uint16_t sdk_major;
    uint16_t sdk_minor;
    uint16_t sdk_patch;
    uint16_t reserved;
    uint64_t flags;
} ContinuityEngineCapabilities;
typedef struct ContinuityEnginePosition { uint32_t line; uint32_t byte_in_line; } ContinuityEnginePosition;
typedef struct ContinuityEngineDelta { size_t at; size_t removed_bytes; size_t inserted_bytes; } ContinuityEngineDelta;
typedef struct ContinuityEngineString { uint8_t *data; size_t len; } ContinuityEngineString;
typedef struct ContinuityEngineUtf16String { uint16_t *data; size_t len; } ContinuityEngineUtf16String;
typedef void (*ContinuityEngineChangeCallback)(void *user_data, uint64_t revision);

CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_capabilities(ContinuityEngineCapabilities *out);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_create_utf8(uint16_t requested_abi_major, const uint8_t *text, size_t text_len, uint64_t revision, ContinuityEngineHandle **out);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_create_utf16(uint16_t requested_abi_major, const uint16_t *text, size_t text_len, uint64_t revision, ContinuityEngineHandle **out);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_destroy(ContinuityEngineHandle *handle);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_set_change_callback(ContinuityEngineHandle *handle, ContinuityEngineChangeCallback callback, void *user_data);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_set_carets(ContinuityEngineHandle *handle, const ContinuityEnginePosition *carets, size_t caret_count);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_insert_utf8(ContinuityEngineHandle *handle, const uint8_t *text, size_t text_len, int64_t timestamp_ms);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_insert_utf16(ContinuityEngineHandle *handle, const uint16_t *text, size_t text_len, int64_t timestamp_ms);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_delete_backward(ContinuityEngineHandle *handle, int64_t timestamp_ms);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_undo(ContinuityEngineHandle *handle, int64_t timestamp_ms);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_redo(ContinuityEngineHandle *handle, int64_t timestamp_ms);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_redo_alternate(ContinuityEngineHandle *handle, int64_t timestamp_ms);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_snapshot_utf8(const ContinuityEngineHandle *handle, ContinuityEngineString *out);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_snapshot_utf16(const ContinuityEngineHandle *handle, ContinuityEngineUtf16String *out);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_revision(const ContinuityEngineHandle *handle, uint64_t *out);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_carets(const ContinuityEngineHandle *handle, ContinuityEnginePosition **out_data, size_t *out_len);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_deltas_since(const ContinuityEngineHandle *handle, uint64_t since_revision, ContinuityEngineDelta **out_data, size_t *out_len);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_string_free(ContinuityEngineString value);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_utf16_string_free(ContinuityEngineUtf16String value);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_carets_free(ContinuityEnginePosition *data, size_t len);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_deltas_free(ContinuityEngineDelta *data, size_t len);
CONTINUITY_ENGINE_API ContinuityEngineStatus continuity_engine_last_error_utf8(uint8_t *buffer, size_t capacity, size_t *out_required);

#ifdef __cplusplus
}
#endif

#endif
