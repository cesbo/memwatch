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
    thread::sleep,
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

    // Spawn child, inherit stdio so you see its output
    let mut cmd_iter = args.command.iter();
    let prog = cmd_iter.next().unwrap();
    let child_args: Vec<&str> = cmd_iter.map(|s| s.as_str()).collect();

    let mut child = Command::new(prog)
        .args(&child_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("failed to spawn `{}`: {}", prog, e)))?;

    let pid = child.id() as i32;
    let interval = Duration::from_millis(args.interval);
    let start = Instant::now();

    // Hide cursor during monitoring
    print!("{}", cursor::Hide);
    io::stdout().flush().ok();

    // Ensure cursor is shown on exit
    let _guard = CursorGuard;

    loop {
        // Check if process exited
        if let Some(status) = child.try_wait()? {
            // Print a final line with exit status
            let (rss, vsz) = meminfo(pid).unwrap_or((0, 0));
            let line = format_status_line(start.elapsed(), rss, vsz);
            print!("\r{}{}", clear::CurrentLine, line);
            io::stdout().flush().ok();
            println!();
            eprintln!("Process exited with status: {}", status);
            break;
        }

        // Sample memory
        let (rss, vsz) = meminfo(pid).unwrap_or((0, 0));

        // Render single updating line
        let line = format_status_line(start.elapsed(), rss, vsz);

        // Clear line and print new content
        print!("\r{}{}", clear::CurrentLine, line);
        io::stdout().flush().ok();

        sleep(interval);
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
