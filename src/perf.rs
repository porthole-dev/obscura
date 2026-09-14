// SPDX-License-Identifier: GPL-3.0-or-later
//! Opt-in timing. With OBSCURA_PERF=1 in the environment, milestones go to
//! stderr as `obscura-perf <ms since main> <event> <detail>`; without it every
//! mark is a single atomic load.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static ON: AtomicBool = AtomicBool::new(false);

pub fn init() {
    START.get_or_init(Instant::now);
    if std::env::var_os("OBSCURA_PERF").is_some() {
        ON.store(true, Ordering::Relaxed);
        mark("main", format_args!("exec_to_main_ms={}", exec_to_main_ms().unwrap_or(-1.0)));
    }
}

#[inline]
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

pub fn ms() -> f64 {
    START.get().map_or(0.0, |s| s.elapsed().as_secs_f64() * 1e3)
}

pub fn mark(event: &str, detail: std::fmt::Arguments) {
    eprintln!("obscura-perf {:.1} {event} {detail}", ms());
}

/// `perf!("event")` or `perf!("event", "k={}", v)`; free when timing is off.
#[macro_export]
macro_rules! perf {
    ($event:expr) => {
        if $crate::perf::on() {
            $crate::perf::mark($event, format_args!(""))
        }
    };
    ($event:expr, $($arg:tt)*) => {
        if $crate::perf::on() {
            $crate::perf::mark($event, format_args!($($arg)*))
        }
    };
}

/// Resident memory in MiB.
pub fn rss_mib() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("VmRSS:").map(|v| v.trim().trim_end_matches("kB").trim().parse::<f64>().ok())))
        .flatten()
        .map_or(0.0, |kb| kb / 1024.0)
}

/// How long the kernel had the process before main ran (10 ms resolution).
fn exec_to_main_ms() -> Option<f64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // Fields after the parenthesised command name; starttime is field 22.
    let start_ticks: f64 = stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()?;
    let uptime: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
    Some(uptime * 1e3 - start_ticks * 10.0)
}
