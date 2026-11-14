//! The sandbox of a plugin process: Landlock (files, and TCP on newer kernels) plus a seccomp
//! allowlist, both through raw system calls (no extra crate, no system library).
//!
//! Everything that allocates or can fail for lack of support is prepared in the parent
//! (`prepare`): if Landlock or seccomp is missing the plugin is not started
//! (`SandboxUnavailable`, fail-closed). Only plain system calls run in the child between
//! `fork` and `exec` (`SandboxPlan::enforce`).
//!
//! What it enforces for a plugin process (the filter is written for statically linked
//! programs; a dynamically linked plugin cannot load its libraries and does not start):
//! - Landlock: the plugin may read and execute files below its own plugin folder and nothing
//!   else; no file can be written, created, removed, renamed or truncated anywhere. With
//!   Landlock ABI 4 or newer TCP bind/connect are forbidden as well; ABI 6 or newer also
//!   scopes abstract Unix sockets and signals; ABI 9 or newer forbids connecting to Unix
//!   sockets by path.
//! - seccomp (allowlist, default answer `EPERM`): no `socket` (only `socketpair` for
//!   `AF_UNIX`), no `execve`, `execveat` only for the one descriptor the host starts, no
//!   `fork`/`vfork` (threads only), `tgkill` only against itself, no `ptrace`, `mount`,
//!   `unshare`, `setns`, `bpf`, `kexec_*`, `*_module`, `memfd_create`, `io_uring`, ... because
//!   none of them is on the list.
//! - `no_new_privs`, resource limits (see `host`), all inherited descriptors closed.
//!
//! Not enforced: how much CPU/memory a plugin uses within its limits; side channels; kernel
//! bugs. Only x86-64 Linux is implemented; elsewhere the sandbox is unavailable.

use crate::error::{PluginError, PluginResult};

use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

/// What the running kernel can enforce (for the "Plugins" dialog: nothing is hidden).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SandboxInfo {
    pub landlock_abi: u32,
    /// TCP bind/connect are also stopped by Landlock (otherwise only by seccomp).
    pub landlock_net: bool,
    /// Abstract Unix sockets and signals are scoped by Landlock.
    pub landlock_scoped: bool,
}

/// Custom errno values the child reports through the exec error pipe.
pub(crate) const FAIL_SANDBOX: i32 = 4_000_001;
pub(crate) const FAIL_SETUP: i32 = 4_000_002;
/// The executable no longer matches what was verified (changed in place).
pub(crate) const FAIL_CHANGED: i32 = 4_000_003;

#[cfg(target_arch = "x86_64")]
mod imp {
    use super::*;

    const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
    const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
    const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;
    const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;
    const LANDLOCK_RULE_PATH_BENEATH: u32 = 1;

    const FS_EXECUTE: u64 = 1 << 0;
    const FS_READ_FILE: u64 = 1 << 2;
    const FS_READ_DIR: u64 = 1 << 3;
    const FS_V1_ALL: u64 = (1 << 13) - 1; // bits 0..=12
    const FS_REFER: u64 = 1 << 13;
    const FS_TRUNCATE: u64 = 1 << 14;
    const FS_IOCTL_DEV: u64 = 1 << 15;
    const FS_RESOLVE_UNIX: u64 = 1 << 16;
    const NET_BIND_TCP: u64 = 1 << 0;
    const NET_CONNECT_TCP: u64 = 1 << 1;
    const SCOPE_ABSTRACT_UNIX: u64 = 1 << 0;
    const SCOPE_SIGNAL: u64 = 1 << 1;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
        handled_access_net: u64,
        scoped: u64,
    }

    #[repr(C, packed)]
    struct PathBeneath {
        allowed_access: u64,
        parent_fd: i32,
    }

    const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
    const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_SET_MODE_FILTER: libc::c_long = 1;
    const SECCOMP_GET_ACTION_AVAIL: libc::c_long = 2;

    const BPF_LD_W_ABS: u16 = 0x20;
    const BPF_ALU_AND_K: u16 = 0x54;
    const BPF_JEQ_K: u16 = 0x15;
    const BPF_RET_K: u16 = 0x06;

    const OFF_NR: u32 = 0;
    const OFF_ARCH: u32 = 4;
    const fn off_arg(index: u32) -> u32 {
        16 + 8 * index
    }

    const CLONE_THREAD: u32 = 0x0001_0000;
    const CLONE_NS_MASK: u32 = 0x0002_0000 // NEWNS
        | 0x0200_0000 // NEWCGROUP
        | 0x0400_0000 // NEWUTS
        | 0x0800_0000 // NEWIPC
        | 0x1000_0000 // NEWUSER
        | 0x2000_0000 // NEWPID
        | 0x4000_0000 // NEWNET
        | 0x0000_0080; // NEWTIME
    const AT_EMPTY_PATH: u32 = 0x1000;

    fn stmt(code: u16, k: u32) -> libc::sock_filter {
        libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k,
        }
    }
    fn jump(code: u16, k: u32, jt: u8, jf: u8) -> libc::sock_filter {
        libc::sock_filter { code, jt, jf, k }
    }
    fn ret_errno(errno: i32) -> libc::sock_filter {
        stmt(BPF_RET_K, SECCOMP_RET_ERRNO | errno as u32)
    }

    /// A syscall number with its own small block that ends in a verdict on every path. The
    /// block is skipped when the number does not match, with the number still in the
    /// accumulator.
    fn block(prog: &mut Vec<libc::sock_filter>, nr: libc::c_long, body: Vec<libc::sock_filter>) {
        prog.push(jump(BPF_JEQ_K, nr as u32, 0, body.len() as u8));
        prog.extend(body);
    }

    fn allowlist() -> Vec<libc::c_long> {
        use libc::*;
        vec![
            SYS_read,
            SYS_write,
            SYS_readv,
            SYS_writev,
            SYS_pread64,
            SYS_pwrite64,
            SYS_preadv,
            SYS_pwritev,
            SYS_preadv2,
            SYS_pwritev2,
            SYS_close,
            SYS_close_range,
            SYS_fstat,
            SYS_stat,
            SYS_lstat,
            SYS_newfstatat,
            SYS_statx,
            SYS_lseek,
            SYS_mmap,
            SYS_mprotect,
            SYS_munmap,
            SYS_mremap,
            SYS_brk,
            SYS_madvise,
            SYS_rt_sigaction,
            SYS_rt_sigprocmask,
            SYS_rt_sigreturn,
            SYS_sigaltstack,
            SYS_access,
            SYS_faccessat,
            SYS_faccessat2,
            SYS_pipe,
            SYS_pipe2,
            SYS_dup,
            SYS_dup2,
            SYS_dup3,
            SYS_fcntl,
            SYS_poll,
            SYS_ppoll,
            SYS_select,
            SYS_pselect6,
            SYS_epoll_create,
            SYS_epoll_create1,
            SYS_epoll_ctl,
            SYS_epoll_wait,
            SYS_epoll_pwait,
            SYS_epoll_pwait2,
            SYS_eventfd,
            SYS_eventfd2,
            SYS_nanosleep,
            SYS_clock_nanosleep,
            SYS_clock_gettime,
            SYS_clock_getres,
            SYS_gettimeofday,
            SYS_time,
            SYS_getpid,
            SYS_gettid,
            SYS_getppid,
            SYS_getuid,
            SYS_geteuid,
            SYS_getgid,
            SYS_getegid,
            SYS_uname,
            SYS_getcwd,
            SYS_readlink,
            SYS_readlinkat,
            SYS_open,
            SYS_openat,
            SYS_getdents64,
            SYS_futex,
            SYS_set_tid_address,
            SYS_set_robust_list,
            SYS_get_robust_list,
            SYS_rseq,
            SYS_arch_prctl,
            SYS_sched_yield,
            SYS_sched_getaffinity,
            SYS_getrlimit,
            SYS_getrandom,
            SYS_exit,
            SYS_exit_group,
            SYS_sysinfo,
            SYS_membarrier,
            SYS_restart_syscall,
            SYS_getrusage,
            SYS_times,
            SYS_umask,
            SYS_mincore,
        ]
    }

    /// Prctl options a runtime needs: thread names, anonymous memory names, no_new_privs.
    const PRCTL_ALLOWED: [u32; 5] = [15, 16, 0x5356_4d41, 38, 39];
    /// ioctl requests: terminal attributes, bytes available, close-on-exec flags.
    const IOCTL_ALLOWED: [u32; 4] = [0x5401, 0x541b, 0x5451, 0x5450];

    /// The seccomp program and the indices of instructions whose constant is the process id
    /// (set in the child, after `fork`).
    pub(super) fn build_filter(exe_fd: i32) -> (Vec<libc::sock_filter>, [usize; 2]) {
        let mut prog = vec![
            stmt(BPF_LD_W_ABS, OFF_ARCH),
            jump(BPF_JEQ_K, AUDIT_ARCH_X86_64, 1, 0),
            stmt(BPF_RET_K, 0x8000_0000), // kill the process on a foreign architecture
            stmt(BPF_LD_W_ABS, OFF_NR),
        ];
        // threads only (CLONE_THREAD), never in a new namespace
        block(
            &mut prog,
            libc::SYS_clone,
            vec![
                stmt(BPF_LD_W_ABS, off_arg(0)),
                stmt(BPF_ALU_AND_K, CLONE_NS_MASK),
                jump(BPF_JEQ_K, 0, 0, 4),
                stmt(BPF_LD_W_ABS, off_arg(0)),
                stmt(BPF_ALU_AND_K, CLONE_THREAD),
                jump(BPF_JEQ_K, CLONE_THREAD, 0, 1),
                stmt(BPF_RET_K, SECCOMP_RET_ALLOW),
                ret_errno(libc::EPERM),
            ],
        );
        // the C library falls back to `clone` on ENOSYS
        block(&mut prog, libc::SYS_clone3, vec![ret_errno(libc::ENOSYS)]);
        // the one exec the host performs, on the verified descriptor
        block(
            &mut prog,
            libc::SYS_execveat,
            vec![
                stmt(BPF_LD_W_ABS, off_arg(4)),
                jump(BPF_JEQ_K, AT_EMPTY_PATH, 0, 3),
                stmt(BPF_LD_W_ABS, off_arg(0)),
                jump(BPF_JEQ_K, exe_fd as u32, 0, 1),
                stmt(BPF_RET_K, SECCOMP_RET_ALLOW),
                ret_errno(libc::EPERM),
            ],
        );
        // unnamed local pipes between its own threads, no sockets to anything
        block(
            &mut prog,
            libc::SYS_socketpair,
            vec![
                stmt(BPF_LD_W_ABS, off_arg(0)),
                jump(BPF_JEQ_K, libc::AF_UNIX as u32, 0, 1),
                stmt(BPF_RET_K, SECCOMP_RET_ALLOW),
                ret_errno(libc::EPERM),
            ],
        );
        // signals only to itself (`abort`, `raise`)
        let pid_slot = prog.len() + 1 /* nr jump */ + 1 /* ld */;
        block(
            &mut prog,
            libc::SYS_tgkill,
            vec![
                stmt(BPF_LD_W_ABS, off_arg(0)),
                jump(BPF_JEQ_K, 0, 0, 1), // constant patched with the process id
                stmt(BPF_RET_K, SECCOMP_RET_ALLOW),
                ret_errno(libc::EPERM),
            ],
        );
        // Reading our limits is needed by runtimes; setting limits could harm the host.
        let limit_pid_slot = prog.len() + 3;
        block(
            &mut prog,
            libc::SYS_prlimit64,
            vec![
                stmt(BPF_LD_W_ABS, off_arg(0)),
                jump(BPF_JEQ_K, 0, 1, 0),
                jump(BPF_JEQ_K, 0, 0, 5), // constant patched with the process id
                stmt(BPF_LD_W_ABS, off_arg(2)),
                jump(BPF_JEQ_K, 0, 0, 3),
                stmt(BPF_LD_W_ABS, off_arg(2) + 4),
                jump(BPF_JEQ_K, 0, 0, 1),
                stmt(BPF_RET_K, SECCOMP_RET_ALLOW),
                ret_errno(libc::EPERM),
            ],
        );
        for (nr, allowed, arg) in [
            (libc::SYS_prctl, &PRCTL_ALLOWED[..], 0),
            (libc::SYS_ioctl, &IOCTL_ALLOWED[..], 1),
        ] {
            let mut body = vec![stmt(BPF_LD_W_ABS, off_arg(arg))];
            for option in allowed {
                body.push(jump(BPF_JEQ_K, *option, 0, 1));
                body.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
            }
            body.push(ret_errno(libc::EPERM));
            block(&mut prog, nr, body);
        }
        for nr in allowlist() {
            prog.push(jump(BPF_JEQ_K, nr as u32, 0, 1));
            prog.push(stmt(BPF_RET_K, SECCOMP_RET_ALLOW));
        }
        prog.push(ret_errno(libc::EPERM));
        (prog, [pid_slot, limit_pid_slot])
    }

    fn landlock_abi() -> Option<u32> {
        // SAFETY: plain system call with a null attribute, asks for the ABI version.
        let version = unsafe {
            libc::syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                std::ptr::null::<RulesetAttr>(),
                0usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            )
        };
        (version >= 1).then_some(version as u32)
    }

    fn seccomp_available() -> bool {
        let action = SECCOMP_RET_ERRNO;
        // SAFETY: asks the kernel whether the action exists; the pointer is valid.
        unsafe {
            libc::syscall(
                libc::SYS_seccomp,
                SECCOMP_GET_ACTION_AVAIL,
                0,
                &action as *const u32,
            ) == 0
        }
    }

    pub(super) fn probe() -> PluginResult<SandboxInfo> {
        let abi = landlock_abi().ok_or(PluginError::SandboxUnavailable)?;
        if !seccomp_available() {
            return Err(PluginError::SandboxUnavailable);
        }
        Ok(SandboxInfo {
            landlock_abi: abi,
            landlock_net: abi >= 4,
            landlock_scoped: abi >= 6,
        })
    }

    pub struct SandboxPlan {
        ruleset: OwnedFd,
        filter: Vec<libc::sock_filter>,
        pid_slot: [usize; 2],
        pub info: SandboxInfo,
    }

    pub(super) fn prepare(plugin_dir: BorrowedFd<'_>, exe_fd: i32) -> PluginResult<SandboxPlan> {
        let info = probe()?;
        let abi = info.landlock_abi;
        let mut fs = FS_V1_ALL;
        if abi >= 2 {
            fs |= FS_REFER;
        }
        if abi >= 3 {
            fs |= FS_TRUNCATE;
        }
        if abi >= 5 {
            fs |= FS_IOCTL_DEV;
        }
        if abi >= 9 {
            fs |= FS_RESOLVE_UNIX;
        }
        let attr = RulesetAttr {
            handled_access_fs: fs,
            handled_access_net: if abi >= 4 {
                NET_BIND_TCP | NET_CONNECT_TCP
            } else {
                0
            },
            scoped: if abi >= 6 {
                SCOPE_ABSTRACT_UNIX | SCOPE_SIGNAL
            } else {
                0
            },
        };
        let size = if abi >= 6 {
            24
        } else if abi >= 4 {
            16
        } else {
            8
        };
        // SAFETY: `attr` is a valid, initialized struct of at least `size` bytes.
        let fd = unsafe {
            libc::syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                &attr as *const RulesetAttr,
                size as usize,
                0u32,
            )
        };
        if fd < 0 {
            return Err(PluginError::SandboxUnavailable);
        }
        // SAFETY: the kernel returned a new descriptor that nobody else owns.
        let ruleset = unsafe { OwnedFd::from_raw_fd(fd as i32) };
        let rule = PathBeneath {
            allowed_access: (FS_EXECUTE | FS_READ_FILE | FS_READ_DIR) & fs,
            parent_fd: plugin_dir.as_raw_fd(),
        };
        // SAFETY: valid ruleset descriptor, valid packed rule struct, no flags.
        let added = unsafe {
            libc::syscall(
                SYS_LANDLOCK_ADD_RULE,
                ruleset.as_raw_fd(),
                LANDLOCK_RULE_PATH_BENEATH,
                &rule as *const PathBeneath,
                0u32,
            )
        };
        if added != 0 {
            return Err(PluginError::SandboxUnavailable);
        }
        let (filter, pid_slot) = build_filter(exe_fd);
        Ok(SandboxPlan {
            ruleset,
            filter,
            pid_slot,
            info,
        })
    }

    impl SandboxPlan {
        /// Runs in the child between `fork` and `exec`: only plain system calls, no
        /// allocation. Any failure means the plugin must not run.
        pub(crate) unsafe fn enforce(&mut self) -> Result<(), i32> {
            // SAFETY (whole function): raw system calls with valid arguments owned by `self`.
            if libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, self.ruleset.as_raw_fd(), 0u32) != 0 {
                return Err(FAIL_SANDBOX);
            }
            for slot in self.pid_slot {
                self.filter[slot].k = libc::getpid() as u32;
            }
            let program = libc::sock_fprog {
                len: self.filter.len() as u16,
                filter: self.filter.as_mut_ptr(),
            };
            if libc::syscall(
                libc::SYS_seccomp,
                SECCOMP_SET_MODE_FILTER,
                0,
                &program as *const libc::sock_fprog,
            ) != 0
            {
                return Err(FAIL_SANDBOX);
            }
            Ok(())
        }
    }

    #[cfg(test)]
    pub(super) fn test_filter_len() -> usize {
        build_filter(5).0.len()
    }
}

#[cfg(not(target_arch = "x86_64"))]
mod imp {
    use super::*;

    pub struct SandboxPlan {
        pub info: SandboxInfo,
    }

    pub(super) fn probe() -> PluginResult<SandboxInfo> {
        Err(PluginError::SandboxUnavailable)
    }

    pub(super) fn prepare(_: BorrowedFd<'_>, _: i32) -> PluginResult<SandboxPlan> {
        Err(PluginError::SandboxUnavailable)
    }

    impl SandboxPlan {
        pub(crate) unsafe fn enforce(&mut self) -> Result<(), i32> {
            Err(FAIL_SANDBOX)
        }
    }
}

pub use imp::SandboxPlan;

/// What this system can enforce; `SandboxUnavailable` if Landlock or seccomp is missing.
pub fn probe() -> PluginResult<SandboxInfo> {
    imp::probe()
}

/// Builds the sandbox for a plugin process (parent side). The plugin folder is the only place
/// the plugin may read and execute from.
pub(crate) fn prepare(plugin_dir: BorrowedFd<'_>, exe_fd: i32) -> PluginResult<SandboxPlan> {
    imp::prepare(plugin_dir, exe_fd)
}

#[cfg(all(test, target_arch = "x86_64"))]
mod tests {
    #[test]
    fn prlimit_only_allows_getters_for_self() {
        let (mut filter, slots) = super::imp::build_filter(-1);
        // SAFETY: the child uses only syscalls and stack storage after fork, then _exit.
        // No filter is installed in the test runner or its other threads.
        unsafe {
            let parent = libc::getpid();
            let mut new = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            // A regressed filter must not change the test runner's actual limits.
            assert_eq!(libc::getrlimit(libc::RLIMIT_NOFILE, &mut new), 0);
            let child = libc::fork();
            assert!(child >= 0);
            if child == 0 {
                let own = libc::getpid();
                for slot in slots {
                    filter[slot].k = own as u32;
                }
                let program = libc::sock_fprog {
                    len: filter.len() as u16,
                    filter: filter.as_mut_ptr(),
                };
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0
                    || libc::syscall(libc::SYS_seccomp, 1, 0, &program) != 0
                {
                    libc::_exit(1);
                }
                let mut old = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                for pid in [0, own] {
                    if libc::syscall(
                        libc::SYS_prlimit64,
                        pid,
                        libc::RLIMIT_NOFILE,
                        std::ptr::null::<libc::rlimit>(),
                        &mut old,
                    ) != 0
                    {
                        libc::_exit(2);
                    }
                }
                for (pid, new_limit) in [
                    (parent, &new as *const libc::rlimit),
                    (0, &new as *const libc::rlimit),
                    (own, &new as *const libc::rlimit),
                    (parent, std::ptr::null()),
                    (0, (1usize << 32) as *const libc::rlimit),
                ] {
                    if libc::syscall(
                        libc::SYS_prlimit64,
                        pid,
                        libc::RLIMIT_NOFILE,
                        new_limit,
                        &mut old,
                    ) != -1
                        || *libc::__errno_location() != libc::EPERM
                    {
                        libc::_exit(3);
                    }
                }
                libc::_exit(0);
            }
            let mut status = 0;
            assert_eq!(libc::waitpid(child, &mut status, 0), child);
            assert_eq!(status, 0, "sandbox child failed: {status}");
        }
    }

    #[test]
    fn the_filter_is_bounded() {
        // a seccomp program may have at most 4096 instructions
        assert!(super::imp::test_filter_len() < 1000);
    }
}
