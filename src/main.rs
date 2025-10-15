use std::{
    collections::HashMap,
    io,
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

#[derive(Parser, Debug)]
#[command(
    name = "memwatch",
    about = "Run a command and watch its memory (Linux)"
)]
struct Args {
    /// Update interval in milliseconds
    #[arg(short, long, default_value_t = 500)]
    interval: u64,

    /// Unit for printing: auto|kb|mb|gb
    #[arg(long, default_value = "auto")]
    unit: String,

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

    let mut last_print_len = 0usize;

    loop {
        // Check if process exited
        if let Some(status) = child.try_wait()? {
            // Print a final line with exit status
            let (rss, vsz) = meminfo(pid).unwrap_or((0, 0));
            let line = format_status_line(&args.unit, start.elapsed(), rss, vsz);
            clear_line(last_print_len);
            println!("{}", line);
            eprintln!("Process exited with status: {}", status);
            break;
        }

        // Sample memory
        let (rss, vsz) = meminfo(pid).unwrap_or((0, 0));

        // Render single updating line
        let line = format_status_line(&args.unit, start.elapsed(), rss, vsz);

        // Overwrite the same line in-place
        print!("\r{}", line);
        // Track printed length to clear leftovers on the last line
        last_print_len = line.len();
        use std::io::Write;
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

fn format_status_line(unit: &str, elapsed: Duration, rss_bytes: u64, vsz_bytes: u64) -> String {
    let (rss_val, rss_unit) = format_bytes_unit(rss_bytes, unit);
    let (vsz_val, vsz_unit) = format_bytes_unit(vsz_bytes, unit);
    let (mm, ss) = (elapsed.as_secs() / 60, elapsed.as_secs() % 60);

    format!(
        "[{:02}:{:02}] RSS: {:.2} {} | VSZ: {:.2} {}",
        mm, ss, rss_val, rss_unit, vsz_val, vsz_unit
    )
}

fn clear_line(prev_len: usize) {
    if prev_len > 0 {
        print!("\r{:width$}\r", "", width = prev_len);
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

/// Format to chosen unit (auto/kb/mb/gb)
fn format_bytes_unit(bytes: u64, unit: &str) -> (f64, &'static str) {
    match unit.to_ascii_lowercase().as_str() {
        "kb" => (bytes as f64 / 1024.0, "KB"),
        "mb" => (bytes as f64 / (1024.0 * 1024.0), "MB"),
        "gb" => (bytes as f64 / (1024.0 * 1024.0 * 1024.0), "GB"),
        _ => {
            // auto
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
    }
}
