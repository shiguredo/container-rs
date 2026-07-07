#ifndef XPC_BRIDGE_H
#define XPC_BRIDGE_H

#include <xpc/xpc.h>
#include <stdbool.h>
#include <stdint.h>

xpc_connection_t xpc_bridge_create_connection(const char *service_name);
xpc_object_t xpc_bridge_create_dictionary(void);
void xpc_bridge_dictionary_set_string(xpc_object_t dict, const char *key, const char *val);
void xpc_bridge_dictionary_set_data(xpc_object_t dict, const char *key, const void *data, size_t len);
void xpc_bridge_dictionary_set_int64(xpc_object_t dict, const char *key, int64_t val);
void xpc_bridge_dictionary_set_uint64(xpc_object_t dict, const char *key, uint64_t val);
void xpc_bridge_dictionary_set_bool(xpc_object_t dict, const char *key, bool val);
// 成功時 true。xpc_fd_create 失敗時はキー未設定のまま false。
bool xpc_bridge_dictionary_set_fd(xpc_object_t dict, const char *key, int fd);

xpc_object_t xpc_bridge_send_message_with_reply_sync(xpc_connection_t connection, xpc_object_t message, uint64_t timeout_ns);

const void *xpc_bridge_copy_data(xpc_object_t dict, const char *key, size_t *len);
const char *xpc_bridge_get_string(xpc_object_t dict, const char *key);
// キーが存在し型が XPC_TYPE_INT64 のときだけ *out に書き出して true。
bool xpc_bridge_get_int64_checked(xpc_object_t dict, const char *key, int64_t *out);
bool xpc_bridge_is_error(xpc_object_t obj);
bool xpc_bridge_get_error_description(xpc_object_t obj, char *buf, size_t buf_len);

int xpc_bridge_get_log_fds(xpc_object_t reply, int *fds, size_t max_fds);

void xpc_bridge_release(xpc_object_t obj);
void xpc_bridge_connection_cancel(xpc_connection_t conn);

#endif
