// C API of the Feather Scan Rust core (crates/feather-core/src/ffi.rs).
#pragma once
#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/// Receives one JSON event. Called on a background thread.
typedef void (*feather_event_cb)(const char *json, void *ctx);

void *feather_start(const char *config_json, feather_event_cb callback, void *ctx);
bool feather_set_config(void *handle, const char *config_json);
void feather_scan_now(void *handle);
void feather_stop(void *handle);
char *feather_discover(const char *ip, uint32_t timeout_ms);
char *feather_default_config(void);
void feather_free_string(char *s);

#ifdef __cplusplus
}
#endif
