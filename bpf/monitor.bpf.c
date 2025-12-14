#include <linux/bpf.h>
#include <bpf/bpf_helpers.h>

char LICENSE[] SEC("license") = "GPL";

struct event {
    __u32 pid;
    char comm[16];
    char filename[256];
};

struct {
    __uint(type, BPF_MAP_TYPE_PERF_EVENT_ARRAY);
    __uint(key_size, sizeof(__u32));
    __uint(value_size, sizeof(__u32));
} events SEC(".maps");

static __always_inline int is_hdas_path(const char *path) {
    #pragma unroll
    for (int i = 0; i < 240; i++) {
        if (path[i] == '\0')
            break;
        if (path[i] == '/' && path[i+1] == 'h' && path[i+2] == 'd' &&
            path[i+3] == 'a' && path[i+4] == 's' && (path[i+5] == '/' || path[i+5] == '\0'))
            return 1;
    }
    return 0;
}

static __always_inline int is_target_path(const char *path) {
    #pragma unroll
    for (int i = 0; i < 200; i++) {
        if (path[i] == '\0')
            break;

        if (path[i] == '/' && path[i+1] == '.') {
            if (path[i+2] == 'c' && path[i+3] == 'a' && path[i+4] == 'c' &&
                path[i+5] == 'h' && path[i+6] == 'e' && (path[i+7] == '/' || path[i+7] == '\0'))
                return 1;

            if (path[i+2] == 'l' && path[i+3] == 'o' && path[i+4] == 'c' &&
                path[i+5] == 'a' && path[i+6] == 'l' && (path[i+7] == '/' || path[i+7] == '\0'))
                return 1;

            if (path[i+2] == 'c' && path[i+3] == 'o' && path[i+4] == 'n' &&
                path[i+5] == 'f' && path[i+6] == 'i' && path[i+7] == 'g' && (path[i+8] == '/' || path[i+8] == '\0'))
                return 1;
        }
    }

    if (path[0] == '.') {
        if (path[1] == 'c' && path[2] == 'a' && path[3] == 'c' &&
            path[4] == 'h' && path[5] == 'e' && (path[6] == '/' || path[6] == '\0'))
            return 1;
        if (path[1] == 'l' && path[2] == 'o' && path[3] == 'c' &&
            path[4] == 'a' && path[5] == 'l' && (path[6] == '/' || path[6] == '\0'))
            return 1;
        if (path[1] == 'c' && path[2] == 'o' && path[3] == 'n' &&
            path[4] == 'f' && path[5] == 'i' && path[6] == 'g' && (path[7] == '/' || path[7] == '\0'))
            return 1;
    }

    return 0;
}

SEC("tracepoint/syscalls/sys_enter_openat")
int trace_openat(void *ctx) {
    struct event e = {};

    __u64 pid_tgid = bpf_get_current_pid_tgid();
    e.pid = pid_tgid >> 32;

    bpf_get_current_comm(&e.comm, sizeof(e.comm));

    void *filename_ptr;
    bpf_probe_read(&filename_ptr, sizeof(filename_ptr), ctx + 24);
    bpf_probe_read_user_str(&e.filename, sizeof(e.filename), filename_ptr);

    if (is_target_path(e.filename) && !is_hdas_path(e.filename)) {
        bpf_perf_event_output(ctx, &events, BPF_F_CURRENT_CPU, &e, sizeof(e));
    }

    return 0;
}
