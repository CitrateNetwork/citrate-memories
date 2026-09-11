//! The per-session MCP shim: what each Claude Code instance actually spawns.
//! A pure stdio ⇄ Unix-socket proxy onto the `mcp_serve` daemon, so any number
//! of sessions share the one process that holds the RocksDB lock.
//!
//! .mcp.json points here:
//!   command: .../target/release/examples/mcp_connect
//!   args:    [<socket-path>, <db-path>]
//!
//! If the daemon isn't running, the shim starts it (the sibling `mcp_serve`
//! binary next to this one) and retries the connect. Two shims racing both
//! spawn a daemon; the loser exits at the RocksDB open, the winner binds the
//! socket — so the race is safe and needs no extra lock. Daemon stderr goes to
//! `<socket-path>.log`.

#[cfg(unix)]
fn main() {
    use std::io::Write;

    let sock = match std::env::args().nth(1) {
        Some(s) => s,
        None => {
            eprintln!("usage: mcp_connect <socket-path> <db-path>");
            std::process::exit(2);
        }
    };
    let db = match std::env::args().nth(2) {
        Some(s) => s,
        None => {
            eprintln!("usage: mcp_connect <socket-path> <db-path>");
            std::process::exit(2);
        }
    };

    let stream = match connect_or_start(&sock, &db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mcp_connect: cannot reach daemon on {sock}: {e}");
            std::process::exit(1);
        }
    };

    // socket → stdout (responses) on a thread; stdin → socket (requests) here.
    let mut sock_read = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mcp_connect: cannot clone stream: {e}");
            std::process::exit(1);
        }
    };
    let out = std::thread::spawn(move || {
        let mut stdout = std::io::stdout();
        let _ = std::io::copy(&mut sock_read, &mut stdout);
        let _ = stdout.flush();
    });

    let mut sock_write = stream;
    let mut stdin = std::io::stdin();
    let _ = std::io::copy(&mut stdin, &mut sock_write);
    // Our client closed stdin: half-close so the daemon ends this session, then
    // drain any in-flight responses before exiting.
    let _ = sock_write.shutdown(std::net::Shutdown::Write);
    let _ = out.join();
}

/// Connect to the daemon socket; if nobody is listening, spawn the sibling
/// `mcp_serve` binary and retry (it may spend a few seconds loading the model).
#[cfg(unix)]
fn connect_or_start(sock: &str, db: &str) -> std::io::Result<std::os::unix::net::UnixStream> {
    use std::os::unix::net::UnixStream;

    if let Ok(s) = UnixStream::connect(sock) {
        return Ok(s);
    }

    let me = std::env::current_exe()?;
    let serve_bin = me.with_file_name("mcp_serve");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{sock}.log"))?;
    std::process::Command::new(&serve_bin)
        .arg(db)
        .arg(sock)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log)
        .spawn()?;

    // Model load can take a few seconds (more on first weight download).
    let mut last_err = std::io::Error::other("daemon never came up");
    for _ in 0..300 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        match UnixStream::connect(sock) {
            Ok(s) => return Ok(s),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

// ---- Windows: the same shim over a namespaced pipe (interprocess) ----
//
// The endpoint name is derived by the SAME `mem_mcp::endpoint_name` rule the
// daemon binds with, so the basename of `<socket-path>` selects the pipe. There
// is no half-close on a named pipe, so end-of-input is signalled by dropping the
// send half; the daemon's per-connection reader then reaches EOF and the session
// ends. Windows is verified by the team.
#[cfg(windows)]
fn main() {
    use interprocess::local_socket::traits::Stream as _;

    let sock = match std::env::args().nth(1) {
        Some(s) => s,
        None => {
            eprintln!("usage: mcp_connect <socket-path> <db-path>");
            std::process::exit(2);
        }
    };
    let db = match std::env::args().nth(2) {
        Some(s) => s,
        None => {
            eprintln!("usage: mcp_connect <socket-path> <db-path>");
            std::process::exit(2);
        }
    };

    let stream = match connect_or_start(&sock, &db) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("mcp_connect: cannot reach daemon on {sock}: {e}");
            std::process::exit(1);
        }
    };

    // Full-duplex pipe split into owned halves: pipe → stdout on a thread,
    // stdin → pipe here.
    let (mut sock_read, mut sock_write) = stream.split();
    let out = std::thread::spawn(move || {
        let mut stdout = std::io::stdout();
        let _ = std::io::copy(&mut sock_read, &mut stdout);
        use std::io::Write as _;
        let _ = stdout.flush();
    });

    let mut stdin = std::io::stdin();
    let _ = std::io::copy(&mut stdin, &mut sock_write);
    // Our client closed stdin: drop the send half so the daemon reaches EOF and
    // ends this session (named pipes have no `shutdown(Write)`), then drain any
    // in-flight responses before exiting.
    drop(sock_write);
    let _ = out.join();
}

/// Connect to the daemon pipe; if nobody is listening, spawn the sibling
/// `mcp_serve` binary and retry (it may spend a few seconds loading the model).
#[cfg(windows)]
fn connect_or_start(sock: &str, db: &str) -> std::io::Result<interprocess::local_socket::Stream> {
    use interprocess::local_socket::traits::Stream as _;
    use interprocess::local_socket::Stream;

    if let Ok(s) = Stream::connect(mem_mcp::endpoint_name(sock)?) {
        return Ok(s);
    }

    let me = std::env::current_exe()?;
    let serve_bin = me.with_file_name("mcp_serve");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{sock}.log"))?;
    std::process::Command::new(&serve_bin)
        .arg(db)
        .arg(sock)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log)
        .spawn()?;

    // Model load can take a few seconds (more on first weight download).
    let mut last_err = std::io::Error::other("daemon never came up");
    for _ in 0..300 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        match Stream::connect(mem_mcp::endpoint_name(sock)?) {
            Ok(s) => return Ok(s),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

#[cfg(not(any(unix, windows)))]
fn main() {
    eprintln!("mcp_connect: local sockets unsupported on this platform; use mcp_stdio");
    std::process::exit(1);
}
