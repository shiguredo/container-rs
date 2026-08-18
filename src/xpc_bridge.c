#include "xpc_bridge.h"
#include <dispatch/dispatch.h>
#include <stdatomic.h>
#include <stdint.h>
#include <string.h>
#include <stdlib.h>

xpc_connection_t xpc_bridge_create_connection(const char *service_name) {
    xpc_connection_t conn = xpc_connection_create_mach_service(service_name, NULL, 0);
    if (!conn) return NULL;
    xpc_connection_set_event_handler(conn, ^(xpc_object_t _){});
    xpc_connection_activate(conn);
    return conn;
}

void xpc_bridge_connection_cancel(xpc_connection_t conn) {
    if (conn) xpc_connection_cancel(conn);
}

// 契約は xpc_bridge.h 参照 (失敗時 NULL を返す経路がある)。
xpc_object_t xpc_bridge_create_dictionary(void) {
    return xpc_dictionary_create(NULL, NULL, 0);
}

void xpc_bridge_dictionary_set_string(xpc_object_t dict, const char *key, const char *val) {
    xpc_dictionary_set_string(dict, key, val);
}

void xpc_bridge_dictionary_set_data(xpc_object_t dict, const char *key, const void *data, size_t len) {
    xpc_dictionary_set_data(dict, key, data, len);
}

void xpc_bridge_dictionary_set_int64(xpc_object_t dict, const char *key, int64_t val) {
    xpc_dictionary_set_int64(dict, key, val);
}

void xpc_bridge_dictionary_set_uint64(xpc_object_t dict, const char *key, uint64_t val) {
    xpc_dictionary_set_uint64(dict, key, val);
}

void xpc_bridge_dictionary_set_bool(xpc_object_t dict, const char *key, bool val) {
    xpc_dictionary_set_bool(dict, key, val);
}

bool xpc_bridge_dictionary_set_fd(xpc_object_t dict, const char *key, int fd) {
    xpc_object_t xfd = xpc_fd_create(fd);
    if (!xfd) {
        return false;
    }
    xpc_dictionary_set_value(dict, key, xfd);
    xpc_release(xfd);
    return true;
}

void xpc_bridge_release(xpc_object_t obj) {
    if (obj) xpc_release(obj);
}

// 応答待ちの共有コンテキスト。
// 待機側 (呼び出し元) と reply ブロックの双方が参照を持ち、後に手放した方が解放する。
// XPC の reply ハンドラは (接続 cancel 時のエラー通知を含め) 必ず 1 回呼ばれるため、
// タイムアウト後に遅れて届いた reply もここで確実に release される。
// 以前の実装はタイムアウト時に解放済みセマフォへ signal する use-after-free があった。
typedef struct {
    _Atomic int refs;
    xpc_object_t result;
    dispatch_semaphore_t sem;
} xpc_bridge_reply_ctx;

static void xpc_bridge_reply_ctx_unref(xpc_bridge_reply_ctx *ctx) {
    if (atomic_fetch_sub_explicit(&ctx->refs, 1, memory_order_acq_rel) == 1) {
        if (ctx->result) xpc_release(ctx->result);
        dispatch_release(ctx->sem);
        free(ctx);
    }
}

xpc_object_t xpc_bridge_send_message_with_reply_sync(
    xpc_connection_t connection,
    xpc_object_t message,
    uint64_t timeout_ns)
{
    xpc_bridge_reply_ctx *ctx = calloc(1, sizeof(*ctx));
    if (!ctx) return NULL;
    atomic_store_explicit(&ctx->refs, 2, memory_order_relaxed);
    ctx->sem = dispatch_semaphore_create(0);
    // calloc 失敗と同じく NULL を返す。reply 未登録・sem 未設定のため unref は呼ばない。
    if (!ctx->sem) {
        free(ctx);
        return NULL;
    }

    xpc_connection_send_message_with_reply(connection, message, NULL, ^(xpc_object_t reply) {
        if (reply) ctx->result = xpc_retain(reply);
        dispatch_semaphore_signal(ctx->sem);
        xpc_bridge_reply_ctx_unref(ctx);
    });

    // timeout_ns == 0 は「即時タイムアウト」として扱う (Duration の意味論に合わせる)。
    // INT64_MAX 超過は負のオフセット (= 即時タイムアウト) になるのを避けるためクランプする。
    dispatch_time_t timeout = DISPATCH_TIME_NOW;
    if (timeout_ns != 0) {
        int64_t delta_ns = timeout_ns > (uint64_t)INT64_MAX
            ? INT64_MAX
            : (int64_t)timeout_ns;
        timeout = dispatch_time(DISPATCH_TIME_NOW, delta_ns);
    }

    xpc_object_t out = NULL;
    if (dispatch_semaphore_wait(ctx->sem, timeout) == 0) {
        // reply ブロックは signal の前に result を書き終えているため、ここで所有権を引き取れる。
        out = ctx->result;
        ctx->result = NULL;
    }
    xpc_bridge_reply_ctx_unref(ctx);
    return out;
}

const void *xpc_bridge_copy_data(xpc_object_t dict, const char *key, size_t *len) {
    size_t length = 0;
    const void *data = xpc_dictionary_get_data(dict, key, &length);
    if (!data || length == 0) { if (len) *len = 0; return NULL; }
    void *copy = malloc(length);
    if (copy) memcpy(copy, data, length);
    if (len) *len = length;
    return copy;
}

const char *xpc_bridge_get_string(xpc_object_t dict, const char *key) {
    return xpc_dictionary_get_string(dict, key);
}

bool xpc_bridge_get_int64_checked(xpc_object_t dict, const char *key, int64_t *out) {
    if (!dict || !key || !out) {
        return false;
    }
    xpc_object_t val = xpc_dictionary_get_value(dict, key);
    if (!val || xpc_get_type(val) != XPC_TYPE_INT64) {
        return false;
    }
    *out = xpc_int64_get_value(val);
    return true;
}

bool xpc_bridge_is_error(xpc_object_t obj) {
    return xpc_get_type(obj) == XPC_TYPE_ERROR;
}

bool xpc_bridge_get_error_description(xpc_object_t obj, char *buf, size_t buf_len) {
    if (!xpc_bridge_is_error(obj)) return false;
    const char *desc = xpc_dictionary_get_string(obj, _xpc_error_key_description);
    if (!desc) return false;
    strlcpy(buf, desc, buf_len);
    return true;
}

int xpc_bridge_get_log_fds(xpc_object_t reply, int *fds, size_t max_fds) {
    xpc_object_t arr = xpc_dictionary_get_value(reply, "logs");
    if (!arr || xpc_get_type(arr) != XPC_TYPE_ARRAY) return -1;
    size_t count = xpc_array_get_count(arr);
    if (count > max_fds) count = max_fds;
    for (size_t i = 0; i < count; i++) {
        fds[i] = xpc_array_dup_fd(arr, i);
    }
    return (int)count;
}
