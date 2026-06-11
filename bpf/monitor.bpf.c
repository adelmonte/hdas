#include <linux/bpf.h>
#include <bpf/bpf_helpers.h>

char LICENSE[] SEC("license") = "GPL";

#define MAX_PATTERNS 16
#define PATTERN_LEN 64

struct event {
    __u32 pid;
    __s32 dfd;
    char comm[16];
    char filename[256];
};

struct pattern {
    char prefix[PATTERN_LEN];
};

// Path prefixes to match, populated from the config by userspace before
// the programs are attached. Zeroed entries are unused.
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, MAX_PATTERNS);
    __type(key, __u32);
    __type(value, struct pattern);
} patterns SEC(".maps");

struct {
    __uint(type, BPF_MAP_TYPE_PERF_EVENT_ARRAY);
    __uint(key_size, sizeof(__u32));
    __uint(value_size, sizeof(__u32));
} events SEC(".maps");

// Argument layout shared by sys_enter_openat and sys_enter_openat2:
// 8 bytes of common tracepoint fields, the syscall number, then the
// syscall arguments at 8-byte offsets (dfd, filename, ...).
struct openat_args {
    unsigned long long common;
    long syscall_nr;
    long dfd;
    const char *filename;
};

static __always_inline int matches_pattern(const char *p) {
    for (__u32 i = 0; i < MAX_PATTERNS; i++) {
        __u32 key = i;
        struct pattern *pat = bpf_map_lookup_elem(&patterns, &key);
        if (!pat || pat->prefix[0] == '\0')
            continue;
        int ok = 1;
        for (int j = 0; j < PATTERN_LEN; j++) {
            char c = pat->prefix[j];
            if (c == '\0')
                break;
            if (p[j] != c) {
                ok = 0;
                break;
            }
        }
        if (ok)
            return 1;
    }
    return 0;
}

static __always_inline int is_hdas(const char *p) {
    for (int i = 0; i < 200; i++) {
        if (p[i] == '\0') return 0;
        if (p[i] == '/' && p[i+1] == 'h' && p[i+2] == 'd' && p[i+3] == 'a' && p[i+4] == 's'
            && (p[i+5] == '/' || p[i+5] == '\0')) return 1;
    }
    return 0;
}

static __always_inline int handle_open(void *ctx, long dfd, const char *uptr) {
    struct event e = {};

    e.pid = bpf_get_current_pid_tgid() >> 32;
    e.dfd = (__s32)dfd;
    bpf_get_current_comm(&e.comm, sizeof(e.comm));
    if (bpf_probe_read_user_str(&e.filename, sizeof(e.filename), uptr) <= 0)
        return 0;

    if (!matches_pattern(e.filename))
        return 0;
    if (is_hdas(e.filename))
        return 0;

    bpf_perf_event_output(ctx, &events, BPF_F_CURRENT_CPU, &e, sizeof(e));
    return 0;
}

SEC("tracepoint/syscalls/sys_enter_openat")
int trace_openat(struct openat_args *ctx) {
    return handle_open(ctx, ctx->dfd, ctx->filename);
}

SEC("tracepoint/syscalls/sys_enter_openat2")
int trace_openat2(struct openat_args *ctx) {
    return handle_open(ctx, ctx->dfd, ctx->filename);
}
