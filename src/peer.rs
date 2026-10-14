//! Who is on the other end of a socket connection (#12).
//!
//! The daemon lets a connection answer permission requests only if it is the
//! same user, running the very file the daemon runs, with a process tree that
//! can be read to the top and holds no agent. Hooks apply the same check to
//! the daemon, so a fake daemon can't answer for it. This stops an agent from
//! approving its own request with `nc -U`, a script, a tapin it launched, or a
//! server of its own on the socket. It can't stop a same-user process that goes
//! out of its way (detaching from the agent and driving tapin in a pty, ptrace,
//! editing the agent's settings): it guards against accidents and prompt
//! injection, not a determined local attacker.

use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

/// Agent executables (by file name) whose descendants may never answer.
const AGENT_NAMES: &[&str] = &["claude", "codex", "gemini"];

/// How far up the process tree to look; deeper trees are refused.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct Peer {
    pub uid: u32,
    /// The peer's executable, as the OS reports it.
    pub exe: Option<PathBuf>,
    /// That executable's file identity (device, inode).
    pub exe_id: Option<(u64, u64)>,
    /// The peer's parent, grandparent, … up to and including pid 1, by
    /// process name (readable for anyone's process, unlike the executable).
    pub ancestors: Vec<(u32, Option<String>)>,
    /// False if the walk couldn't reach pid 1 (too deep, or a process
    /// couldn't be read): an agent could be hiding above.
    pub complete: bool,
}

/// A binary trusted as tapin: its path, and the identity of the file the
/// daemon started from, so another program written to that path won't do.
#[derive(Debug, Clone, PartialEq)]
pub struct Trusted {
    pub path: PathBuf,
    pub id: Option<(u64, u64)>,
}

/// Why a peer may not answer.
pub type Refusal = &'static str;

/// Checks a peer against the rules above. `trusted` are the tapin binaries,
/// `agent_pids` the agent processes the daemon knows of.
pub fn verify(
    peer: &Peer,
    uid: u32,
    trusted: &[Trusted],
    agent_pids: &[u32],
) -> Result<(), Refusal> {
    if peer.uid != uid {
        return Err("another user");
    }
    let exe = peer
        .exe
        .as_deref()
        .map(normalize)
        .ok_or("unknown program")?;
    let same_file = |t: &Trusted| t.id.is_some() && t.id == peer.exe_id;
    if !trusted
        .iter()
        .any(|t| normalize(&t.path) == exe && same_file(t))
    {
        return Err("not the tapin binary");
    }
    if !peer.complete {
        return Err("process tree not fully readable");
    }
    for (pid, name) in &peer.ancestors {
        if agent_pids.contains(pid) {
            return Err("started by an agent");
        }
        // A process we can't even name could be an agent.
        let name = name.as_deref().ok_or("process tree not fully readable")?;
        if is_agent_name(name) {
            return Err("started by an agent");
        }
    }
    Ok(())
}

/// An agent's process name, with or without an extension: the kernel calls
/// Claude Code's native binary `claude.exe`.
fn is_agent_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    AGENT_NAMES.contains(&stem)
}

/// The real path, without Linux's " (deleted)" mark on a replaced binary.
fn normalize(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let path = Path::new(text.strip_suffix(" (deleted)").unwrap_or(&text));
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The process at the other end of `stream`, from the OS.
pub fn of(stream: &UnixStream) -> Option<Peer> {
    let (pid, uid) = peer_pid_uid(stream.as_raw_fd())?;
    let mut ancestors = Vec::new();
    // pid 1 itself (e.g. tapin as a container's init) has nothing above it.
    let mut complete = pid == 1;
    let mut next = if complete { None } else { parent_of(pid) };
    while let Some(at) = next {
        // 0 is no process (outside our pid namespace): the walk can't finish.
        if at == 0 || ancestors.len() >= MAX_DEPTH {
            break;
        }
        ancestors.push((at, name_of(at)));
        if at == 1 {
            complete = true;
            break;
        }
        next = parent_of(at);
    }
    Some(Peer {
        uid,
        exe: exe_of(pid),
        exe_id: exe_id_of(pid),
        ancestors,
        complete,
    })
}

/// A process's short name (what `ps -o comm` shows), readable for any process.
#[cfg(target_os = "linux")]
fn name_of(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim_end().to_string())
}

#[cfg(target_os = "macos")]
fn name_of(pid: u32) -> Option<String> {
    let info = short_info(pid)?;
    let bytes: Vec<u8> = info
        .pbsi_comm
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    String::from_utf8(bytes).ok().filter(|n| !n.is_empty())
}

/// The identity of the file a process runs. On Linux `/proc/<pid>/exe`
/// still reaches the original file after its path was replaced; on macOS it
/// is the file now at the process's path (a known, weaker approximation).
fn exe_id_of(pid: u32) -> Option<(u64, u64)> {
    #[cfg(target_os = "linux")]
    let meta = std::fs::metadata(format!("/proc/{pid}/exe")).ok()?;
    #[cfg(target_os = "macos")]
    let meta = std::fs::metadata(exe_of(pid)?).ok()?;
    Some((meta.dev(), meta.ino()))
}

#[cfg(target_os = "macos")]
fn peer_pid_uid(fd: i32) -> Option<(u32, u32)> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: pid and len are valid for writes of the sizes passed.
    let ok = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: uid and gid are valid out-pointers.
    let ok2 = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    (ok == 0 && ok2 == 0 && pid > 0).then_some((pid as u32, uid))
}

#[cfg(target_os = "linux")]
fn peer_pid_uid(fd: i32) -> Option<(u32, u32)> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: cred and len are valid for writes of the sizes passed.
    let ok = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    (ok == 0 && cred.pid > 0).then_some((cred.pid as u32, cred.uid))
}

#[cfg(target_os = "macos")]
fn exe_of(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: buf is valid for writes of the length passed.
    let n = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(PathBuf::from(std::ffi::OsString::from_vec(buf)))
}

#[cfg(target_os = "linux")]
fn exe_of(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// The short BSD info, which (unlike PROC_PIDTBSDINFO) can be read for
/// root-owned processes such as `login`, above every Terminal/iTerm2 shell.
#[cfg(target_os = "macos")]
fn short_info(pid: u32) -> Option<libc::proc_bsdshortinfo> {
    // SAFETY: proc_bsdshortinfo is plain data; zeroed is a valid value.
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as i32;
    // SAFETY: info is valid for writes of `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdshortinfo).cast(),
            size,
        )
    };
    (n == size && info.pbsi_pid == pid).then_some(info)
}

#[cfg(target_os = "macos")]
fn parent_of(pid: u32) -> Option<u32> {
    Some(short_info(pid)?.pbsi_ppid)
}

#[cfg(target_os = "linux")]
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    parse_ppid(&stat)
}

/// The ppid from a `/proc/<pid>/stat` line. The command name is in
/// parentheses and may itself contain spaces and `)`, so parse after the last `)`.
pub fn parse_ppid(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    // Fields after the name: state, ppid, …
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// `of`, as the daemon and hooks use it. In debug builds only, with
/// `TAPIN_TEST_IGNORE_ANCESTRY` set (see `.cargo/config.toml`), ancestors are
/// left out: the test suite itself often runs under an agent (a developer's
/// Claude Code), which would otherwise refuse every test inbox. The rules
/// stay covered by `verify`'s own tests; release builds have no such switch.
pub fn checked(stream: &UnixStream) -> Option<Peer> {
    #[allow(unused_mut)]
    let mut peer = of(stream)?;
    #[cfg(debug_assertions)]
    if std::env::var_os("TAPIN_TEST_IGNORE_ANCESTRY").is_some() {
        peer.ancestors.clear();
        peer.complete = true;
    }
    Some(peer)
}

/// This process's own binary, as the file it runs right now.
pub fn own_binary() -> Option<Trusted> {
    Some(Trusted {
        path: std::env::current_exe().ok()?,
        id: exe_id_of(std::process::id()),
    })
}

/// The binaries the daemon trusts as tapin: its own, plus (debug builds only,
/// for tests) `TAPIN_TRUST_PEER_EXE`.
pub fn trusted_binaries() -> Vec<Trusted> {
    #[allow(unused_mut)]
    let mut trusted: Vec<Trusted> = own_binary().into_iter().collect();
    #[cfg(debug_assertions)]
    if let Some(p) = std::env::var_os("TAPIN_TRUST_PEER_EXE").filter(|p| !p.is_empty()) {
        let id = std::fs::metadata(&p).ok().map(|m| (m.dev(), m.ino()));
        trusted.push(Trusted {
            path: PathBuf::from(p),
            id,
        });
    }
    trusted
}

/// True if the daemon at the other end of `stream` is this same tapin, not
/// something else listening on the socket (e.g. an agent's fake daemon that
/// approves everything). Hooks only take answers from such a daemon.
pub fn is_genuine_daemon(stream: &UnixStream) -> bool {
    let (Some(own), Some(peer)) = (own_binary(), checked(stream)) else {
        return false;
    };
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    verify(&peer, uid, &[own], &[]).is_ok()
}
