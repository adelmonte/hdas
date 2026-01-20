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

static __always_inline int match_etc(const char *p) {
    return p[0] == '/' && p[1] == 'e' && p[2] == 't' && p[3] == 'c' && p[4] == '/';
}

static __always_inline int match_dot_cache(const char *p) {
    return p[0] == '.' && p[1] == 'c' && p[2] == 'a' && p[3] == 'c' &&
           p[4] == 'h' && p[5] == 'e' && (p[6] == '/' || p[6] == '\0');
}

static __always_inline int match_dot_local(const char *p) {
    return p[0] == '.' && p[1] == 'l' && p[2] == 'o' && p[3] == 'c' &&
           p[4] == 'a' && p[5] == 'l' && (p[6] == '/' || p[6] == '\0');
}

static __always_inline int match_dot_config(const char *p) {
    return p[0] == '.' && p[1] == 'c' && p[2] == 'o' && p[3] == 'n' &&
           p[4] == 'f' && p[5] == 'i' && p[6] == 'g' && (p[7] == '/' || p[7] == '\0');
}

static __always_inline int match_dot_dir(const char *p) {
    // Check for /. prefix (absolute paths like /home/user/.config)
    if (p[0] == '/' && p[1] == '.') {
        return match_dot_cache(p + 1) || match_dot_local(p + 1) || match_dot_config(p + 1);
    }
    // Check for relative paths starting with .cache, .local, .config
    if (p[0] == '.') {
        return match_dot_cache(p) || match_dot_local(p) || match_dot_config(p);
    }
    return 0;
}

static __always_inline int is_hdas(const char *p) {
    for (int i = 0; i < 200; i++) {
        if (p[i] == '\0') return 0;
        if (p[i] == 'h' && p[i+1] == 'd' && p[i+2] == 'a' && p[i+3] == 's') return 1;
    }
    return 0;
}

SEC("tracepoint/syscalls/sys_enter_openat")
int trace_openat(void *ctx) {
    struct event e = {};

    e.pid = bpf_get_current_pid_tgid() >> 32;
    bpf_get_current_comm(&e.comm, sizeof(e.comm));

    void *fname;
    bpf_probe_read(&fname, sizeof(fname), ctx + 24);
    bpf_probe_read_user_str(&e.filename, sizeof(e.filename), fname);

    int matched = match_etc(e.filename);
    if (!matched) {
        for (int i = 0; i < 200; i++) {
            if (e.filename[i] == '\0') break;
            if (match_dot_dir(&e.filename[i])) { matched = 1; break; }
        }
    }

    if (matched && !is_hdas(e.filename)) {
        bpf_perf_event_output(ctx, &events, BPF_F_CURRENT_CPU, &e, sizeof(e));
    }

    return 0;
}
