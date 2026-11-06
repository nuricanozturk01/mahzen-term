//! Background feed for live status-bar widgets: system stats, command output and redraw ticks.
//!
//! One thread sleeps until the next due job and does nothing at all when no visible widget needs data, so a
//! bar without live widgets costs no wakeups. Config reloads send a new [`Plan`] and the thread re-plans.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use iced::futures::channel::mpsc::UnboundedSender;
use mahzenterm_core::bars::{StatusbarConfig, TabbarConfig, WidgetDef, WidgetKind};
use mahzenterm_platform::{Sampler, Stats};

/// Longest a command widget may run before it is killed.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// Smallest delay between two CPU samples that gives a meaningful reading.
const FIRST_CPU_DELAY: Duration = Duration::from_millis(400);
/// Marquee frame period (about 30 fps).
pub const FRAME: Duration = Duration::from_millis(33);
const CLOCK_TICK: Duration = Duration::from_secs(10);
const CLOCK_TICK_SECONDS: Duration = Duration::from_secs(1);
const STATS_TICK: Duration = Duration::from_secs(2);
const COMMAND_TICK: Duration = Duration::from_secs(5);
const MAX_LINE: usize = 400;

/// Something the UI should react to.
#[derive(Debug, Clone)]
pub enum Feed {
    /// Time-dependent widgets (clock, marquee) need a fresh frame.
    Redraw,
    Stats(Stats),
    /// Result line of a command widget (`id`, text).
    Output(String, String),
}

/// What the feed thread has to do; derived from the visible widgets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub stats: Option<Duration>,
    pub redraw: Option<Duration>,
    /// `(widget id, shell line, period)`.
    pub commands: Vec<(String, String, Duration)>,
}

fn secs(def: &WidgetDef, default: Duration) -> Duration {
    def.interval
        .map_or(default, |s| Duration::from_secs_f32(s.max(1.0)))
}

/// Work out the jobs needed by the widgets in the given bars.
pub fn plan(
    tabbar: &TabbarConfig,
    statusbar: &StatusbarConfig,
    widgets: &BTreeMap<String, WidgetDef>,
) -> Plan {
    let mut out = Plan::default();
    let mut ids: Vec<&String> = tabbar.left.iter().chain(&tabbar.right).collect();
    if statusbar.visible {
        ids.extend(
            statusbar
                .left
                .iter()
                .chain(&statusbar.center)
                .chain(&statusbar.right),
        );
    }
    ids.sort();
    ids.dedup();
    let faster = |slot: &mut Option<Duration>, d: Duration| {
        *slot = Some(slot.map_or(d, |c| c.min(d)));
    };
    for id in ids {
        let def = widgets
            .get(id)
            .cloned()
            .or_else(|| WidgetKind::builtin(id).map(WidgetDef::of));
        let Some(def) = def else { continue };
        match def.kind {
            WidgetKind::Cpu | WidgetKind::Memory | WidgetKind::Load | WidgetKind::Uptime => {
                faster(&mut out.stats, secs(&def, STATS_TICK));
            }
            WidgetKind::Clock => {
                let precise = def.format.as_deref().is_some_and(|f| f.contains("%S"));
                faster(
                    &mut out.redraw,
                    if precise {
                        CLOCK_TICK_SECONDS
                    } else {
                        CLOCK_TICK
                    },
                );
            }
            WidgetKind::Text | WidgetKind::Command if def.marquee => faster(&mut out.redraw, FRAME),
            _ => {}
        }
        if def.kind == WidgetKind::Command {
            if let Some(line) = def.command.clone() {
                out.commands
                    .push((id.clone(), line, secs(&def, COMMAND_TICK)));
            }
        }
    }
    out
}

/// Handle to the feed thread; dropping it stops the thread.
pub struct Feeder {
    plans: mpsc::Sender<Plan>,
}

impl Feeder {
    pub fn spawn(out: UnboundedSender<Feed>) -> Self {
        let (plans, rx) = mpsc::channel::<Plan>();
        thread::Builder::new()
            .name("mahzenterm-feed".into())
            .spawn(move || run(&rx, &out))
            .ok();
        Self { plans }
    }

    /// Replace the jobs; the thread runs every job once right away.
    pub fn set(&self, plan: Plan) {
        let _ = self.plans.send(plan);
    }
}

struct Job {
    next: Instant,
}

fn run(rx: &mpsc::Receiver<Plan>, out: &UnboundedSender<Feed>) {
    let mut plan = Plan::default();
    let mut sampler: Option<Sampler> = None;
    let mut stats_job = Job {
        next: Instant::now(),
    };
    let mut redraw_job = Job {
        next: Instant::now(),
    };
    let mut command_jobs: HashMap<String, Job> = HashMap::new();
    let busy: Arc<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>> = Arc::default();
    loop {
        let now = Instant::now();
        let due = [
            plan.stats.map(|_| stats_job.next),
            plan.redraw.map(|_| redraw_job.next),
        ]
        .into_iter()
        .flatten()
        .chain(command_jobs.values().map(|j| j.next))
        .min();
        let wait = due.map_or(Duration::from_secs(3600), |d| {
            d.saturating_duration_since(now)
        });
        match rx.recv_timeout(wait) {
            Ok(new) => {
                let now = Instant::now();
                if new.stats.is_some() && sampler.is_none() {
                    sampler = Some(Sampler::new());
                    stats_job.next = now + FIRST_CPU_DELAY;
                } else if new.stats != plan.stats {
                    stats_job.next = now;
                }
                redraw_job.next = now;
                command_jobs.retain(|id, _| new.commands.iter().any(|(c, _, _)| c == id));
                for (id, _, _) in &new.commands {
                    command_jobs.entry(id.clone()).or_insert(Job { next: now });
                }
                plan = new;
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let now = Instant::now();
        if let Some(every) = plan.stats.filter(|_| stats_job.next <= now) {
            if let Some(s) = sampler.as_mut() {
                if out.unbounded_send(Feed::Stats(s.sample())).is_err() {
                    return;
                }
            }
            stats_job.next = now + every;
        }
        if let Some(every) = plan.redraw.filter(|_| redraw_job.next <= now) {
            if out.unbounded_send(Feed::Redraw).is_err() {
                return;
            }
            redraw_job.next = now + every;
        }
        for (id, line, every) in &plan.commands {
            let Some(job) = command_jobs.get_mut(id) else {
                continue;
            };
            if job.next > now {
                continue;
            }
            job.next = now + *every;
            let flag = busy
                .lock()
                .map(|mut m| m.entry(id.clone()).or_default().clone())
                .unwrap_or_default();
            if flag.swap(true, Ordering::AcqRel) {
                continue; // the previous run is still going
            }
            let (id, line, out) = (id.clone(), line.clone(), out.clone());
            thread::spawn(move || {
                let text = run_line(&line);
                flag.store(false, Ordering::Release);
                let _ = out.unbounded_send(Feed::Output(id, text));
            });
        }
    }
}

/// Run `line` and return its first non-empty output line, or a short reason it failed.
fn run_line(line: &str) -> String {
    let mut cmd = mahzenterm_platform::shell_command(line);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else {
        return "⚠ cannot start".into();
    };
    let Some(stdout) = child.stdout.take() else {
        return "⚠ no output".into();
    };
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.take(4096).read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let got = rx.recv_timeout(COMMAND_TIMEOUT);
    let _ = child.kill();
    let _ = child.wait();
    match got {
        Ok(buf) => first_line(&String::from_utf8_lossy(&buf)),
        Err(_) => "⚠ timed out".into(),
    }
}

fn first_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(String::new, |l| l.chars().take(MAX_LINE).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs(items: &[(&str, WidgetDef)]) -> BTreeMap<String, WidgetDef> {
        items
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    #[test]
    fn bare_bars_need_no_feed() {
        let tab = TabbarConfig::default();
        let status = StatusbarConfig {
            left: vec!["status".into()],
            right: vec!["theme".into()],
            ..StatusbarConfig::default()
        };
        assert_eq!(plan(&tab, &status, &BTreeMap::new()), Plan::default());
    }

    #[test]
    fn default_bars_poll_stats_and_clock_slowly() {
        let p = plan(
            &TabbarConfig::default(),
            &StatusbarConfig::default(),
            &BTreeMap::new(),
        );
        assert_eq!(p.stats, Some(STATS_TICK));
        assert_eq!(p.redraw, Some(CLOCK_TICK));
        assert!(p.commands.is_empty());
    }

    #[test]
    fn hidden_status_bar_costs_nothing() {
        let status = StatusbarConfig {
            visible: false,
            ..StatusbarConfig::default()
        };
        assert_eq!(
            plan(&TabbarConfig::default(), &status, &BTreeMap::new()),
            Plan::default()
        );
    }

    #[test]
    fn marquee_and_seconds_clock_pick_the_fastest_redraw() {
        let mut news = WidgetDef::of(WidgetKind::Text);
        news.marquee = true;
        let mut clock = WidgetDef::of(WidgetKind::Clock);
        clock.format = Some("%H:%M:%S".into());
        let status = StatusbarConfig {
            center: vec!["news".into()],
            right: vec!["clock".into()],
            ..StatusbarConfig::default()
        };
        let p = plan(
            &TabbarConfig::default(),
            &status,
            &defs(&[("news", news), ("clock", clock)]),
        );
        assert_eq!(p.redraw, Some(FRAME));
    }

    #[test]
    fn command_widgets_become_jobs_with_their_interval() {
        let mut gpu = WidgetDef::of(WidgetKind::Command);
        gpu.command = Some("echo 1".into());
        gpu.interval = Some(9.0);
        let status = StatusbarConfig {
            right: vec!["gpu".into()],
            ..StatusbarConfig::default()
        };
        let p = plan(&TabbarConfig::default(), &status, &defs(&[("gpu", gpu)]));
        assert_eq!(
            p.commands,
            vec![("gpu".into(), "echo 1".into(), Duration::from_secs(9))]
        );
    }

    #[test]
    fn first_line_skips_blanks_and_trims() {
        assert_eq!(first_line("\n  \n  hello  \nworld"), "hello");
        assert_eq!(first_line(""), "");
    }

    #[cfg(unix)]
    #[test]
    fn run_line_reads_output_and_reports_timeouts() {
        assert_eq!(run_line("printf 'a b\\nc'"), "a b");
        assert_eq!(run_line("true"), "");
    }
}
