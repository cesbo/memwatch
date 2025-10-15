use std::{
    collections::HashMap,
    io::{
        self,
        Write,
    },
    process::{
        Command,
        Stdio,
    },
    sync::{
        atomic::{
            AtomicBool,
            Ordering,
        },
        mpsc,
        Arc,
    },
    thread,
    time::{
        Duration,
        Instant,
    },
};

use clap::Parser;
use procfs::process::{
    all_processes,
    Process,
};
use termion::{
    clear,
    cursor,
};

enum OutputMsg {
    Stdout(String),
    Stderr(String),
}

#[derive(Parser, Debug)]
#[command(
    name = "memwatch",
    about = "Run a command and watch its memory (Linux)"
)]
#[command(version)]
struct Args {
    /// Update interval in milliseconds
    #[arg(short, long, default_value_t = 1000)]
    interval: u64,

    /// Command to run (everything after `--`)
    #[arg(trailing_var_arg = true, required = true)]
    command: Vec<String>,
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    // Shared flag for Ctrl+C signal
    let terminated = Arc::new(AtomicBool::new(false));
    let term_flag = terminated.clone();
    ctrlc::set_handler(move || {
        term_flag.store(true, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl+C handler");

    // Spawn child, inherit stdio so you see its output
    let mut cmd_iter = args.command.iter();
    let prog = cmd_iter.next().unwrap();
    let child_args: Vec<&str> = cmd_iter.map(|s| s.as_str()).collect();

    let mut child = Command::new(prog)
        .args(&child_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("failed to spawn `{}`: {}", prog, e)))?;

    let pid = child.id() as i32;
    let interval = Duration::from_millis(args.interval);
    let start = Instant::now();

    // Channel for output lines
    let (tx, rx) = mpsc::channel::<OutputMsg>();

    // Thread reading child's stdout
    if let Some(stdout) = child.stdout.take() {
        let tx_out = tx.clone();
        thread::spawn(move || {
            use std::io::BufRead;
            let reader = io::BufReader::new(stdout);
            for line in reader.lines() {
                if let Ok(l) = line {
                    // Ignore send errors (main thread may have exited)
                    let _ = tx_out.send(OutputMsg::Stdout(l));
                } else {
                    break;
                }
            }
        });
    }

    // Thread reading child's stderr
    if let Some(stderr) = child.stderr.take() {
        let tx_err = tx.clone();
        thread::spawn(move || {
            use std::io::BufRead;
            let reader = io::BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(l) = line {
                    let _ = tx_err.send(OutputMsg::Stderr(l));
                } else {
                    break;
                }
            }
        });
    }

    drop(tx); // Close the original Sender in the main thread

    // Hide cursor during monitoring
    print!("{}", cursor::Hide);
    io::stdout().flush().ok();

    // Ensure cursor is shown on exit
    let _guard = CursorGuard;

    // No need to buffer previously printed non-empty lines; we print immediately
    loop {
        // First, drain all available messages without blocking
        while let Ok(msg) = rx.try_recv() {
            // Before printing a program line, clear the status line
            print!("\r{}", clear::CurrentLine);
            match msg {
                OutputMsg::Stdout(l) => {
                    println!("{}", l);
                }
                OutputMsg::Stderr(l) => {
                    // Visually distinguish stderr
                    eprintln!("{}", l);
                }
            }
        }

        // Check for process termination / Ctrl+C signal
        if terminated.load(Ordering::SeqCst) {
            let _ = child.kill();
        }

        if let Some(status) = child.try_wait()? {
            // Process finished: print final status line and message
            let (rss, vsz) = meminfo(pid).unwrap_or((0, 0));
            let status_line = format_status_line(start.elapsed(), rss, vsz);
            print!("\r{}{}\n", clear::CurrentLine, status_line);
            io::stdout().flush().ok();
            eprintln!(
                "Process exited with status: {}",
                status.code().unwrap_or(-1)
            );
            if terminated.load(Ordering::SeqCst) {
                eprintln!("Interrupted (Ctrl+C)");
            }
            break;
        }

        // Refresh status line on each interval
        let (rss, vsz) = meminfo(pid).unwrap_or((0, 0));
        let status_line = format_status_line(start.elapsed(), rss, vsz);
        print!("\r{}{}", clear::CurrentLine, status_line);
        io::stdout().flush().ok();

        // Wait for interval or a new line (block at most for 'interval')
        match rx.recv_timeout(interval) {
            Ok(msg) => {
                // Got a line before the timer: print it and immediately redraw status
                print!("\r{}", clear::CurrentLine);
                match msg {
                    OutputMsg::Stdout(l) => println!("{}", l),
                    OutputMsg::Stderr(l) => eprintln!("{}", l),
                }
                continue; // Loop back to redraw the status without extra delay
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Nothing arrived – just next tick
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // All reader threads closed – child likely exited; loop will confirm
                continue;
            }
        }
    }

    Ok(())
}

fn meminfo(root_pid: i32) -> procfs::ProcResult<(u64, u64)> {
    let page_size = procfs::page_size();

    let mut children_map: HashMap<i32, Vec<i32>> = HashMap::new();
    for proc in all_processes()? {
        if let Ok(proc) = proc {
            if let Ok(stat) = proc.stat() {
                children_map
                    .entry(stat.ppid)
                    .or_insert_with(Vec::new)
                    .push(stat.pid);
            }
        }
    }

    let mut total_rss = 0u64;
    let mut total_vsz = 0u64;

    let mut stack = vec![root_pid];

    while let Some(pid) = stack.pop() {
        if let Ok(proc) = Process::new(pid) {
            if let Ok(statm) = proc.statm() {
                total_vsz = total_vsz.saturating_add(statm.size * page_size);
                total_rss = total_rss.saturating_add(statm.resident * page_size);
            }
        }

        if let Some(children) = children_map.get(&pid) {
            stack.extend(children);
        }
    }

    Ok((total_rss, total_vsz))
}

/// Guard to ensure cursor is shown on exit (even on panic or Ctrl+C)
struct CursorGuard;

impl Drop for CursorGuard {
    fn drop(&mut self) {
        print!("{}", cursor::Show);
        let _ = io::stdout().flush();
    }
}

fn format_status_line(elapsed: Duration, rss_bytes: u64, vsz_bytes: u64) -> String {
    let (rss_val, rss_unit) = format_bytes_unit(rss_bytes);
    let (vsz_val, vsz_unit) = format_bytes_unit(vsz_bytes);
    let (mm, ss) = (elapsed.as_secs() / 60, elapsed.as_secs() % 60);

    format!(
        "[{:02}:{:02}] RSS: {:.2} {} | VSZ: {:.2} {}",
        mm, ss, rss_val, rss_unit, vsz_val, vsz_unit
    )
}

/// Format to chosen unit (auto/kb/mb/gb)
fn format_bytes_unit(bytes: u64) -> (f64, &'static str) {
    if bytes >= 1024_u64.pow(3) {
        (bytes as f64 / 1024f64.powi(3), "GB")
    } else if bytes >= 1024_u64.pow(2) {
        (bytes as f64 / 1024f64.powi(2), "MB")
    } else if bytes >= 1024 {
        (bytes as f64 / 1024.0, "KB")
    } else {
        (bytes as f64, "B")
    }
}
