//! Deciding a trial boot — the two init scripts it replaces
//! (S04trial, S99ota) as one process with two halves, like the two
//! processes they were: [`guard`] owns the SoC watchdog — feeding
//! every [`STEP`] on its own thread, doing nothing else, so no
//! work anywhere (a log copy, the summary's syncs, the commit's
//! lock wait) can starve a feed — and [`decide`] checks the new
//! image: mestro running stably, the device on a network, the web
//! endpoint answering, until [`HEALTH_DEADLINE`] seconds after
//! boot; then it commits the trial or rolls the board back into
//! the previous slot. The log snapshots live on their own worker
//! thread (the CLI's): the decision only requests one at its
//! decisions, bounded, and never copies a file itself. A boot
//! that never reaches a decision stops feeding at [`GUARD_MAX`]
//! and the watchdog resets into the previous slot.
//!
//! Both halves are pure over their injected worlds
//! ([`TrialWorld`], [`GuardWorld`]) — the tests run without real
//! time. The real worlds are the CLI's; every line logged is the
//! byte the old scripts wrote (the README shows them).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// how often the watchdog is fed and the health checked — the
/// scripts' STEP
pub const STEP: Duration = Duration::from_secs(5);
/// how long mestro has to stay up once first seen before its
/// stability counts — the scripts' SETTLE
pub const SETTLE: Duration = Duration::from_secs(20);
/// give up and roll back this long after boot — the scripts'
/// HEALTH_DEADLINE (seconds since the kernel started: the boot
/// itself takes a while), also for a mestro that has not been
/// stable-checked by then
pub const HEALTH_DEADLINE: u64 = 120;
/// stop feeding without the magic close at this age — the scripts'
/// GUARD_MAX, the hard limit behind the deadline
pub const GUARD_MAX: u64 = 180;

/// everything the decision touches, injected: what it observes
/// (the probes) and what it drives (the summary, the commit, the
/// log). The watchdog is not here — it belongs to the guard thread
/// ([`GuardWorld`]) — and neither are the log copies: the snapshot
/// at a decision is a request to the snapshot worker, bounded, so
/// a copy still running can never hold the decision. The real one
/// is the CLI's; the tests script one
pub trait TrialWorld {
    /// seconds since the kernel started (/proc/uptime)
    fn uptime(&mut self) -> u64;
    /// the pid mestro runs with, if it runs (it appears later than
    /// this process: S50mestro runs after the S04 that starts it)
    fn mestro_pid(&mut self) -> Option<u32>;
    /// an IPv4 network the player is on (an address and a default
    /// route on wlan0 or eth0, or the setup hotspot on wlan0)
    fn network_up(&mut self) -> bool;
    /// mestro's web endpoint answers
    fn web_up(&mut self) -> bool;
    /// wait one step — injected so tests run without real time
    fn sleep(&mut self, step: Duration);
    /// one line into the trial's log (ota.log)
    fn log(&mut self, line: &str);
    /// the final snapshot at a decision: a request to the snapshot
    /// worker, waited for at most a short bound — a copy still
    /// running finishes on its own
    fn snapshot(&mut self);
    /// summary.txt: date, the update status, the result line, the
    /// addresses and routes
    fn write_summary(&mut self, result: &str);
    /// the commit, under the update lock; logs its own outcome
    /// lines — false: it failed (the error is already logged)
    fn commit(&mut self) -> bool;
    /// the rollback reboot, into the previous slot; false: it
    /// failed (the error is already logged)
    fn reboot_rollback(&mut self) -> bool;
}

/// the guard's whole world: the SoC watchdog. `&self`: the world
/// is shared with the decision thread, but the watchdog file is
/// the feed thread's alone — armed once, from the decision thread
/// (so the "no /dev/watchdog" warning keeps its place in the log),
/// then touched by nothing but the loop. The loop does no other
/// work: no mutex beyond that never-contended watchdog, no file
/// IO, no subprocess — nothing between two feeds can outlast the
/// watchdog's window
pub trait GuardWorld {
    /// the trial's age in seconds. The real world captures it once
    /// at arm and derives the rest from its monotonic clock — the
    /// feeding loop reads no file at all; the tests script one
    fn uptime(&self) -> u64;
    /// take the SoC watchdog over; a missing one is this world's
    /// own warning. Called once, from the decision thread, before
    /// the feeding starts
    fn arm(&self);
    /// keep the watchdog from resetting the board — a one-byte
    /// write to the device, the loop's only work
    fn feed(&self);
    /// the magic 'V' and the close: the watchdog stays off
    fn disarm(&self);
    /// one line into the trial's log (ota.log, RAM on the image)
    fn log(&self, line: &str);
    /// wait one step — injected so tests run without real time
    fn sleep(&self, step: Duration);
}

/// the guard as the decision sees it: armed once at the start, and
/// ended by the commit — waited for, so the log order stays the
/// scripts'. A rollback needs no telling: the reboot ends the
/// process, and a failed reboot leaves the feeding to the guard's
/// own deadline
pub trait Guard {
    /// arm the guard: take the SoC watchdog over and start feeding
    fn armed(&self);
    /// the commit landed: the guard writes the magic 'V'
    /// ("trial guard: committed, watchdog off") and ends. False: it
    /// died or ran out of time — no magic close happened, and the
    /// process does not report success. The watchdog then reboots
    /// the board into whatever slot config.txt names: the new one,
    /// if the commit already switched it, else the previous one
    fn committed(&self) -> bool;
}

/// the decision flag the guard polls — the commit is the only
/// thing it needs to learn; everything else it sees on its own
/// clock
#[derive(Clone, Default)]
pub struct GuardSignal {
    committed: Arc<AtomicBool>,
}

impl GuardSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// the commit landed
    pub fn committed(&self) {
        self.committed.store(true, Ordering::SeqCst);
    }

    /// whether the commit landed
    pub fn is_committed(&self) -> bool {
        self.committed.load(Ordering::SeqCst)
    }
}

/// how the guard thread ended — only [`GuardEnd::Disarmed`] wrote
/// the magic close; [`GuardEnd::Expired`] stopped feeding without
/// it, and the watchdog fires
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardEnd {
    /// the commit's magic 'V' closed the watchdog
    Disarmed,
    /// no decision by [`GUARD_MAX`]: the feeding stopped without
    /// the magic close, the watchdog fires
    Expired,
}

/// what the trial boot ended in
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// healthy and committed: the trial stays, the watchdog is off
    Committed,
    /// unhealthy: the board reboots into the previous slot
    RolledBack,
    /// no decision: the caller must park, holding the guard's
    /// watchdog open — it stopped feeding. The reset lands on
    /// whatever slot config.txt names: the previous one, or the
    /// new one if the commit already switched it
    WatchdogReset,
}

/// whether the kernel's route table (the text of /proc/net/route)
/// holds a default route through `iface`. The default route check
/// stays a pure text parse: the table is one small flat file the
/// kernel serves, and this half of the network probe keeps the
/// 5-second loop free of another subprocess — the address half
/// keeps the script's bounded `ip` command (fib_trie has no
/// interface names, parsing it would be the fragile way)
pub fn route_has_default(route_table: &str, iface: &str) -> bool {
    route_table.lines().any(|line| {
        let mut fields = line.split_whitespace();
        fields.next() == Some(iface) && fields.next() == Some("00000000")
    })
}

/// decide one trial boot: check the new image every [`STEP`],
/// commit it healthy — roll it back unhealthy at
/// [`HEALTH_DEADLINE`]. A decision whose action failed (the
/// commit, the rollback reboot) is no decision: the watchdog ends
/// the trial, like the scripts' decider returning while their
/// guard fed on. A guard that died or ran out of time is no
/// decision either — its magic close never happened, so the
/// decision never reports the commit
pub fn decide(world: &mut dyn TrialWorld, guard: &dyn Guard) -> Outcome {
    world.log(&format!(
        "trial boot: guard armed until {GUARD_MAX}s after boot"
    ));
    guard.armed();
    let now = world.uptime();
    world.log(&format!(
        "trial boot: checking health until {HEALTH_DEADLINE}s after boot (now {now}s)"
    ));
    let mut health = Health::WaitingForMestro;
    loop {
        let up = world.uptime();
        health = match step(world, guard, up, health) {
            Step::Health(health) => health,
            Step::Gone(outcome) => return outcome,
        };
        world.sleep(STEP);
    }
}

/// where the health check stands
enum Health {
    /// mestro has not started yet
    WaitingForMestro,
    /// mestro is up; its stability counts from `check_at` (the
    /// [`SETTLE`] settle from first seen)
    Settling { pid: u32, check_at: u64 },
    /// the check loop proper: mestro has to keep its pid
    Checking { pid: u32 },
}

/// what one tick of the health check ended in
enum Step {
    /// the check goes on from here
    Health(Health),
    /// decided — the trial is over
    Gone(Outcome),
}

/// one tick of the health check: the state's next step, with all
/// its effects on `world`
fn step(world: &mut dyn TrialWorld, guard: &dyn Guard, up: u64, health: Health) -> Step {
    match health {
        Health::WaitingForMestro => {
            // the deadline holds here too: a mestro that has not
            // been stable-checked by then never gets the chance
            if up >= HEALTH_DEADLINE {
                return rolled_back(world, "mestro not running stably");
            }
            match world.mestro_pid() {
                Some(pid) => Step::Health(Health::Settling {
                    pid,
                    check_at: up + SETTLE.as_secs(),
                }),
                None => Step::Health(Health::WaitingForMestro),
            }
        }
        Health::Settling { .. } if up >= HEALTH_DEADLINE => {
            rolled_back(world, "mestro not running stably")
        }
        Health::Settling { pid, check_at } if up >= check_at => {
            // the pid has to be the settle's pid on the very first
            // check too — a restart during the settle is unstable,
            // like every later one
            if world.mestro_pid() != Some(pid) {
                rolled_back(world, "mestro not running stably")
            } else {
                checked(world, guard, up, pid)
            }
        }
        settling @ Health::Settling { .. } => Step::Health(settling),
        Health::Checking { pid } if world.mestro_pid() != Some(pid) => {
            rolled_back(world, "mestro not running stably")
        }
        Health::Checking { pid } => checked(world, guard, up, pid),
    }
}

/// one health check of a stable-running mestro: the probes, the
/// log line, and the decision — commit it healthy, or wait for
/// the deadline and roll it back
fn checked(world: &mut dyn TrialWorld, guard: &dyn Guard, up: u64, pid: u32) -> Step {
    let network = world.network_up();
    let web = world.web_up();
    world.log(&format!(
        "trial check at {up}s: mestro=yes network={} web={}",
        yes_no(network),
        yes_no(web)
    ));
    if network && web {
        world.log(&format!("trial boot healthy at {up}s"));
        world.write_summary("healthy");
        if world.commit() {
            // the final snapshot — bounded, and after the commit's
            // own lines are in the log, like the old guard's copy
            world.snapshot();
            if guard.committed() {
                Step::Gone(Outcome::Committed)
            } else {
                // the guard expired or died: no magic close
                // happened — the commit is already on the card, so
                // the watchdog's reboot boots the new slot; only
                // the success report is withheld
                Step::Gone(Outcome::WatchdogReset)
            }
        } else {
            // the commit failed (its error is already logged): no
            // decision, the guard feeds on until its own deadline
            Step::Gone(Outcome::WatchdogReset)
        }
    } else if up >= HEALTH_DEADLINE {
        let reason = if network {
            "web endpoint not answering"
        } else {
            "no network"
        };
        rolled_back(world, reason)
    } else {
        Step::Health(Health::Checking { pid })
    }
}

/// the unhealthy end: the log line, the summary, the final snapshot
/// and the reboot into the previous slot. A reboot that fails is no
/// decision either — the guard feeds on until its own deadline and
/// the watchdog ends the trial
fn rolled_back(world: &mut dyn TrialWorld, reason: &str) -> Step {
    let now = world.uptime();
    world.log(&format!(
        "trial boot unhealthy ({reason}) at {now}s, rolling back"
    ));
    world.write_summary(&format!("unhealthy: {reason}"));
    // the final snapshot — bounded, a copy still running finishes
    // on its own
    world.snapshot();
    if world.reboot_rollback() {
        Step::Gone(Outcome::RolledBack)
    } else {
        Step::Gone(Outcome::WatchdogReset)
    }
}

/// the guard loop — the old S04trial's job, on its own thread, and
/// nothing else: feed the watchdog every [`STEP`], watch the
/// decision flag, and end on the commit with the magic 'V'. No
/// decision by [`GUARD_MAX`]: stop feeding without the magic
/// close, the caller keeps the world (the watchdog file stays
/// open — a close would disarm it) until the parked process dies
/// with the board's reset. A rollback never returns here: the
/// reboot ends the process, and a failed one leaves the feeding
/// to this deadline
pub fn guard(world: &dyn GuardWorld, signal: &GuardSignal) -> GuardEnd {
    loop {
        let up = world.uptime();
        if up >= GUARD_MAX {
            world.log(&format!(
                "trial guard: no decision {GUARD_MAX}s after boot, letting the watchdog reset"
            ));
            // stop feeding without the magic close: the watchdog
            // fires. The file stays open (a close would disarm it) —
            // the caller holds the world until the parked process
            // dies with the board's reset
            return GuardEnd::Expired;
        }
        world.feed();
        if signal.is_committed() {
            world.disarm();
            world.log("trial guard: committed, watchdog off");
            return GuardEnd::Disarmed;
        }
        world.sleep(STEP);
    }
}

/// the guard on its own thread, armed from the decision thread (so
/// the "no /dev/watchdog" warning keeps its place in the log) and
/// ended by the commit. Only a guard that ended [`GuardEnd::Disarmed`]
/// counts as committed: one that expired first or panicked wrote no
/// magic close, so the decision never reports the commit — the
/// watchdog then reboots the board into the slot config.txt names
/// (the new one, if the commit already switched it)
pub struct GuardThread<W: GuardWorld> {
    world: Arc<W>,
    signal: GuardSignal,
    thread: Mutex<Option<JoinHandle<GuardEnd>>>,
}

impl<W: GuardWorld + Send + Sync + 'static> GuardThread<W> {
    pub fn new(world: W) -> Self {
        Self {
            world: Arc::new(world),
            signal: GuardSignal::new(),
            thread: Mutex::new(None),
        }
    }
}

impl<W: GuardWorld + Send + Sync + 'static> Guard for GuardThread<W> {
    fn armed(&self) {
        self.world.arm();
        let (world, signal) = (self.world.clone(), self.signal.clone());
        if let Ok(mut thread) = self.thread.lock() {
            // a thread that cannot start is a guard that never fed:
            // the kernel's own watchdog times out, the safe side
            let _ = std::thread::Builder::new()
                .spawn(move || guard(&*world, &signal))
                .map(|spawned| *thread = Some(spawned));
        }
    }

    fn committed(&self) -> bool {
        self.signal.committed();
        let thread = self.thread.lock().ok().and_then(|mut t| t.take());
        match thread {
            // only a guard that ended disarmed wrote the magic
            // close — one that expired first, or died, did not
            Some(thread) => matches!(thread.join(), Ok(GuardEnd::Disarmed)),
            // never armed: no guard ever fed
            None => false,
        }
    }
}

fn yes_no(up: bool) -> &'static str {
    if up {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the scripted world: probes as functions of the uptime, the
    /// clock the injected sleep advances, every effect recorded
    struct FakeWorld {
        uptime: u64,
        pid: Box<dyn Fn(u64) -> Option<u32>>,
        network: Box<dyn Fn(u64) -> bool>,
        web: Box<dyn Fn(u64) -> bool>,
        commit: bool,
        reboot: bool,
        logs: Vec<String>,
        snapshots: Vec<u64>,
        summaries: Vec<String>,
        commits: u32,
    }

    /// the fake guard: counts what the decision told it
    #[derive(Default)]
    struct FakeGuard {
        armed: std::cell::Cell<u32>,
        commits: std::cell::Cell<u32>,
        /// a dead guard answers its commit with false
        dead: bool,
    }

    impl FakeGuard {
        fn armed_count(&self) -> u32 {
            self.armed.get()
        }

        fn commits_count(&self) -> u32 {
            self.commits.get()
        }
    }

    impl Guard for FakeGuard {
        fn armed(&self) {
            self.armed.set(self.armed.get() + 1);
        }
        fn committed(&self) -> bool {
            self.commits.set(self.commits.get() + 1);
            !self.dead
        }
    }

    /// the uptime the trial starts at (a trial process is S04: ten
    /// seconds into the boot)
    const START: u64 = 10;

    impl FakeWorld {
        /// a world whose mestro runs with pid 100 from the start,
        /// on the network, with its web endpoint up
        fn healthy() -> Self {
            Self::new(|_| Some(100), |_| true, |_| true)
        }

        fn new(
            pid: impl Fn(u64) -> Option<u32> + 'static,
            network: impl Fn(u64) -> bool + 'static,
            web: impl Fn(u64) -> bool + 'static,
        ) -> Self {
            Self {
                uptime: START,
                pid: Box::new(pid),
                network: Box::new(network),
                web: Box::new(web),
                commit: true,
                reboot: true,
                logs: Vec::new(),
                snapshots: Vec::new(),
                summaries: Vec::new(),
                commits: 0,
            }
        }

        /// the two lines every trial boot opens with
        fn opening_logs(&self) -> Vec<String> {
            vec![
                "trial boot: guard armed until 180s after boot".into(),
                format!("trial boot: checking health until 120s after boot (now {START}s)"),
            ]
        }

        /// the check lines the loop writes, from `first` on in
        /// [`STEP`] steps, with the given probe answers
        fn check_logs(first: u64, last: u64, network: bool, web: bool) -> Vec<String> {
            (first..=last)
                .step_by(STEP.as_secs() as usize)
                .map(|up| check_line(up, network, web))
                .collect()
        }
    }

    fn check_line(up: u64, network: bool, web: bool) -> String {
        format!(
            "trial check at {up}s: mestro=yes network={} web={}",
            yes_no(network),
            yes_no(web)
        )
    }

    impl TrialWorld for FakeWorld {
        fn uptime(&mut self) -> u64 {
            self.uptime
        }
        fn mestro_pid(&mut self) -> Option<u32> {
            (self.pid)(self.uptime)
        }
        fn network_up(&mut self) -> bool {
            (self.network)(self.uptime)
        }
        fn web_up(&mut self) -> bool {
            (self.web)(self.uptime)
        }
        fn sleep(&mut self, step: Duration) {
            self.uptime += step.as_secs();
        }
        fn log(&mut self, line: &str) {
            self.logs.push(line.to_string());
        }
        fn snapshot(&mut self) {
            self.snapshots.push(self.uptime);
        }
        fn write_summary(&mut self, result: &str) {
            self.summaries.push(result.to_string());
        }
        fn commit(&mut self) -> bool {
            self.commits += 1;
            self.commit
        }
        fn reboot_rollback(&mut self) -> bool {
            self.reboot
        }
    }

    #[test]
    fn a_healthy_trial_commits() {
        let mut world = FakeWorld::healthy();
        let guard = FakeGuard::default();
        assert_eq!(decide(&mut world, &guard), Outcome::Committed);
        // the settle is 20 s from first seen: the first check runs
        // at 10 + 20
        let mut logs = world.opening_logs();
        logs.push(check_line(30, true, true));
        logs.push("trial boot healthy at 30s".into());
        assert_eq!(world.logs, logs);
        assert_eq!(world.summaries, ["healthy"]);
        assert_eq!(world.commits, 1);
        assert_eq!(guard.armed_count(), 1);
        assert_eq!(guard.commits_count(), 1, "the guard disarms the watchdog");
        // the final snapshot is requested after the commit's line
        // is in the log
        assert_eq!(world.snapshots, [30]);
    }

    #[test]
    fn a_dead_guard_never_reports_the_commit() {
        // the guard died: its magic close never happened, so the
        // decision must not answer Committed. The commit itself
        // succeeded — config.txt names the new slot — so the
        // watchdog's reboot boots it; only the success report is
        // withheld
        let mut world = FakeWorld::healthy();
        let guard = FakeGuard {
            dead: true,
            ..FakeGuard::default()
        };
        assert_eq!(decide(&mut world, &guard), Outcome::WatchdogReset);
        assert_eq!(world.commits, 1, "the commit itself succeeded");
        assert_eq!(guard.commits_count(), 1, "the decision still signaled it");
    }

    #[test]
    fn no_network_rolls_back_at_the_deadline() {
        let mut world = FakeWorld::new(|_| Some(100), |_| false, |_| true);
        let guard = FakeGuard::default();
        assert_eq!(decide(&mut world, &guard), Outcome::RolledBack);
        let mut logs = world.opening_logs();
        logs.extend(FakeWorld::check_logs(30, 120, false, true));
        logs.push("trial boot unhealthy (no network) at 120s, rolling back".into());
        assert_eq!(world.logs, logs);
        assert_eq!(world.summaries, ["unhealthy: no network"]);
        assert_eq!(world.commits, 0);
        assert_eq!(guard.commits_count(), 0, "an unhealthy trial never disarms");
        // the final snapshot is the decision's own request
        assert_eq!(world.snapshots, [120]);
    }

    #[test]
    fn a_mestro_pid_change_rolls_back_right_away() {
        // the network never comes up either, so the check loop
        // keeps running and the pid change is what decides
        let mut world = FakeWorld::new(
            |up| if up < 45 { Some(100) } else { Some(200) },
            |_| false,
            |_| false,
        );
        let guard = FakeGuard::default();
        assert_eq!(decide(&mut world, &guard), Outcome::RolledBack);
        let mut logs = world.opening_logs();
        logs.extend(FakeWorld::check_logs(30, 40, false, false));
        // no check line for the iteration that decided — the
        // scripts broke out before it, too
        logs.push("trial boot unhealthy (mestro not running stably) at 45s, rolling back".into());
        assert_eq!(world.logs, logs);
        assert_eq!(world.summaries, ["unhealthy: mestro not running stably"]);
    }

    #[test]
    fn a_mestro_pid_change_during_the_settle_rolls_back() {
        // the settle sees pid 100, the check loop would see 200:
        // the very first check must catch the restart, like the
        // old script's first comparison
        let mut world = FakeWorld::new(
            |up| if up < 25 { Some(100) } else { Some(200) },
            |_| true,
            |_| true,
        );
        let guard = FakeGuard::default();
        assert_eq!(decide(&mut world, &guard), Outcome::RolledBack);
        let mut logs = world.opening_logs();
        logs.push("trial boot unhealthy (mestro not running stably) at 30s, rolling back".into());
        assert_eq!(world.logs, logs, "no check before the pid is re-checked");
        assert_eq!(world.summaries, ["unhealthy: mestro not running stably"]);
    }

    #[test]
    fn a_mestro_that_never_appears_rolls_back_at_the_deadline() {
        let mut world = FakeWorld::new(|_| None, |_| true, |_| true);
        let guard = FakeGuard::default();
        assert_eq!(decide(&mut world, &guard), Outcome::RolledBack);
        let mut logs = world.opening_logs();
        logs.push("trial boot unhealthy (mestro not running stably) at 120s, rolling back".into());
        assert_eq!(world.logs, logs, "no check before mestro is up");
        assert_eq!(world.summaries, ["unhealthy: mestro not running stably"]);
    }

    #[test]
    fn a_mestro_appearing_too_late_rolls_back_at_the_deadline() {
        // first seen at 110, its settle would end at 130 — past the
        // deadline: it was never stable-checked in time
        let mut world = FakeWorld::new(
            |up| if up >= 110 { Some(100) } else { None },
            |_| true,
            |_| true,
        );
        let guard = FakeGuard::default();
        assert_eq!(decide(&mut world, &guard), Outcome::RolledBack);
        let mut logs = world.opening_logs();
        logs.push("trial boot unhealthy (mestro not running stably) at 120s, rolling back".into());
        assert_eq!(world.logs, logs, "the deadline outruns the settle");
        assert_eq!(world.summaries, ["unhealthy: mestro not running stably"]);
    }

    #[test]
    fn a_mestro_first_seen_at_the_deadline_rolls_back() {
        // the deadline wins over a pid that just appeared: it has
        // not been stable-checked
        let mut world = FakeWorld::new(
            |up| if up >= 120 { Some(100) } else { None },
            |_| true,
            |_| true,
        );
        let guard = FakeGuard::default();
        assert_eq!(decide(&mut world, &guard), Outcome::RolledBack);
        let mut logs = world.opening_logs();
        logs.push("trial boot unhealthy (mestro not running stably) at 120s, rolling back".into());
        assert_eq!(world.logs, logs);
    }

    #[test]
    fn a_web_endpoint_that_never_answers_rolls_back() {
        let mut world = FakeWorld::new(|_| Some(100), |_| true, |_| false);
        let guard = FakeGuard::default();
        assert_eq!(decide(&mut world, &guard), Outcome::RolledBack);
        let mut logs = world.opening_logs();
        logs.extend(FakeWorld::check_logs(30, 120, true, false));
        logs.push("trial boot unhealthy (web endpoint not answering) at 120s, rolling back".into());
        assert_eq!(world.logs, logs);
        assert_eq!(world.summaries, ["unhealthy: web endpoint not answering"]);
    }

    #[test]
    fn a_failed_commit_leaves_the_watchdog_to_end_the_trial() {
        // the health check passed but the commit did not: no
        // decision — the guard is never told and feeds on until its
        // own deadline, like the scripts' decider that returned
        // without one
        let mut world = FakeWorld::healthy();
        world.commit = false;
        let guard = FakeGuard::default();
        assert_eq!(decide(&mut world, &guard), Outcome::WatchdogReset);
        let mut logs = world.opening_logs();
        logs.push(check_line(30, true, true));
        logs.push("trial boot healthy at 30s".into());
        assert_eq!(world.logs, logs);
        assert_eq!(world.commits, 1, "the commit is not retried");
        assert_eq!(world.summaries, ["healthy"]);
        assert_eq!(
            guard.commits_count(),
            0,
            "the guard feeds on, no magic close"
        );
    }

    #[test]
    fn a_failed_rollback_reboot_leaves_the_watchdog_to_end_the_trial() {
        let mut world = FakeWorld::new(|_| Some(100), |_| false, |_| false);
        world.reboot = false;
        let guard = FakeGuard::default();
        assert_eq!(decide(&mut world, &guard), Outcome::WatchdogReset);
        assert_eq!(
            world.logs.last().map(String::as_str),
            Some("trial boot unhealthy (no network) at 120s, rolling back")
        );
        assert_eq!(guard.commits_count(), 0);
    }

    /// the scripted guard world: the clock its injected sleep
    /// advances, every effect recorded
    struct FakeGuardWorld {
        uptime: std::cell::Cell<u64>,
        feeds: std::cell::RefCell<Vec<u64>>,
        logs: std::cell::RefCell<Vec<String>>,
        disarmed: std::cell::Cell<bool>,
        /// the decision flips once this many feeds happened (None:
        /// the decision never comes)
        commit_after_feeds: Option<u32>,
        signal: GuardSignal,
    }

    impl FakeGuardWorld {
        fn new(commit_after_feeds: Option<u32>, signal: GuardSignal) -> Self {
            Self {
                uptime: std::cell::Cell::new(START),
                feeds: std::cell::RefCell::new(Vec::new()),
                logs: std::cell::RefCell::new(Vec::new()),
                disarmed: std::cell::Cell::new(false),
                commit_after_feeds,
                signal,
            }
        }
    }

    impl GuardWorld for FakeGuardWorld {
        fn uptime(&self) -> u64 {
            self.uptime.get()
        }
        fn arm(&self) {}
        fn feed(&self) {
            self.feeds.borrow_mut().push(self.uptime.get());
        }
        fn disarm(&self) {
            self.disarmed.set(true);
        }
        fn log(&self, line: &str) {
            self.logs.borrow_mut().push(line.to_string());
        }
        fn sleep(&self, step: Duration) {
            self.uptime.set(self.uptime.get() + step.as_secs());
            let feeds = self.feeds.borrow().len() as u32;
            if Some(feeds) == self.commit_after_feeds {
                self.signal.committed();
            }
        }
    }

    #[test]
    fn the_guard_disarms_on_the_commit() {
        // the decision lands after the second feed: the guard
        // writes the magic 'V' and ends
        let signal = GuardSignal::new();
        let world = FakeGuardWorld::new(Some(2), signal.clone());
        assert_eq!(guard(&world, &signal), GuardEnd::Disarmed);
        assert_eq!(*world.feeds.borrow(), vec![10, 15, 20]);
        assert!(world.disarmed.get(), "the magic 'V' closed the watchdog");
        assert_eq!(
            *world.logs.borrow(),
            ["trial guard: committed, watchdog off"]
        );
    }

    #[test]
    fn the_guard_gives_up_at_180_without_the_magic_close() {
        // the decision never comes: the feeding stops at 180, the
        // watchdog file stays open (the caller keeps it)
        let signal = GuardSignal::new();
        let world = FakeGuardWorld::new(None, signal.clone());
        assert_eq!(guard(&world, &signal), GuardEnd::Expired);
        assert_eq!(
            *world.feeds.borrow(),
            (10..=175).step_by(5).collect::<Vec<u64>>()
        );
        assert!(!world.disarmed.get(), "no magic close: the watchdog fires");
        assert_eq!(
            *world.logs.borrow(),
            ["trial guard: no decision 180s after boot, letting the watchdog reset"]
        );
    }

    #[test]
    fn a_commit_after_the_guard_expired_is_never_reported() {
        // the guard's deadline ran out while the decision was still
        // working: it stopped feeding without the magic close, and
        // no commit afterwards may count as committed — the
        // decision parks and the watchdog fires
        use std::sync::atomic::AtomicU64;
        let clock = Arc::new(AtomicU64::new(START));
        let disarmed = Arc::new(AtomicBool::new(false));

        struct ExpiredGuard {
            disarmed: Arc<AtomicBool>,
        }
        impl GuardWorld for ExpiredGuard {
            fn uptime(&self) -> u64 {
                // past every deadline on the guard's first turn: it
                // expires at once, long before the decision commits
                1000
            }
            fn arm(&self) {}
            fn feed(&self) {}
            fn disarm(&self) {
                self.disarmed.store(true, Ordering::SeqCst);
            }
            fn log(&self, _line: &str) {}
            fn sleep(&self, _step: Duration) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        struct LateCommitWorld {
            clock: Arc<AtomicU64>,
        }
        impl TrialWorld for LateCommitWorld {
            fn uptime(&mut self) -> u64 {
                self.clock.load(Ordering::SeqCst)
            }
            fn mestro_pid(&mut self) -> Option<u32> {
                Some(100)
            }
            fn network_up(&mut self) -> bool {
                true
            }
            fn web_up(&mut self) -> bool {
                true
            }
            fn sleep(&mut self, step: Duration) {
                self.clock.fetch_add(step.as_secs(), Ordering::SeqCst);
            }
            fn log(&mut self, _line: &str) {}
            fn snapshot(&mut self) {}
            fn write_summary(&mut self, _result: &str) {}
            fn commit(&mut self) -> bool {
                true
            }
            fn reboot_rollback(&mut self) -> bool {
                false
            }
        }

        let guard = GuardThread::new(ExpiredGuard {
            disarmed: disarmed.clone(),
        });
        let mut world = LateCommitWorld { clock };
        assert_ne!(
            decide(&mut world, &guard),
            Outcome::Committed,
            "an expired guard is no successful commit"
        );
        assert!(
            !disarmed.load(Ordering::SeqCst),
            "no magic 'V' was written after the expiry"
        );
    }

    #[test]
    fn a_decision_step_that_blocks_keeps_the_guard_feeding() {
        // a decision step can take longer than the watchdog's 60 s
        // window (the summary's syncs, the commit's lock wait): the
        // guard on its own thread keeps feeding through it — the
        // real thread, against a shared trial clock
        use std::sync::atomic::AtomicU64;
        let clock = Arc::new(AtomicU64::new(START));
        /// (uptime, inside the blocked step) of every feed
        type Feed = (u64, bool);
        let feeds: Arc<Mutex<Vec<Feed>>> = Arc::new(Mutex::new(Vec::new()));
        let in_commit = Arc::new(AtomicBool::new(false));
        let disarmed = Arc::new(AtomicBool::new(false));

        struct ThreadGuard {
            clock: Arc<AtomicU64>,
            feeds: Arc<Mutex<Vec<Feed>>>,
            in_commit: Arc<AtomicBool>,
            disarmed: Arc<AtomicBool>,
        }
        impl GuardWorld for ThreadGuard {
            fn uptime(&self) -> u64 {
                self.clock.load(Ordering::SeqCst)
            }
            fn arm(&self) {}
            fn feed(&self) {
                let feed = (
                    self.clock.load(Ordering::SeqCst),
                    self.in_commit.load(Ordering::SeqCst),
                );
                self.feeds.lock().unwrap().push(feed);
            }
            fn disarm(&self) {
                self.disarmed.store(true, Ordering::SeqCst);
            }
            fn log(&self, _line: &str) {}
            fn sleep(&self, _step: Duration) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        /// a decision world whose commit blocks 70 s of trial time
        struct BlockingWorld {
            clock: Arc<AtomicU64>,
            in_commit: Arc<AtomicBool>,
        }
        impl TrialWorld for BlockingWorld {
            fn uptime(&mut self) -> u64 {
                self.clock.load(Ordering::SeqCst)
            }
            fn mestro_pid(&mut self) -> Option<u32> {
                Some(100)
            }
            fn network_up(&mut self) -> bool {
                true
            }
            fn web_up(&mut self) -> bool {
                true
            }
            fn sleep(&mut self, step: Duration) {
                self.clock.fetch_add(step.as_secs(), Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(1));
            }
            fn log(&mut self, _line: &str) {}
            fn snapshot(&mut self) {}
            fn write_summary(&mut self, _result: &str) {}
            fn commit(&mut self) -> bool {
                // 70 s of trial time pass inside the commit, in
                // small steps so the guard's feeds interleave
                self.in_commit.store(true, Ordering::SeqCst);
                for _ in 0..70 {
                    self.clock.fetch_add(1, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(2));
                }
                self.in_commit.store(false, Ordering::SeqCst);
                true
            }
            fn reboot_rollback(&mut self) -> bool {
                false
            }
        }

        let guard = GuardThread::new(ThreadGuard {
            clock: clock.clone(),
            feeds: feeds.clone(),
            in_commit: in_commit.clone(),
            disarmed: disarmed.clone(),
        });
        let mut world = BlockingWorld {
            clock: clock.clone(),
            in_commit: in_commit.clone(),
        };
        assert_eq!(decide(&mut world, &guard), Outcome::Committed);
        let during = feeds
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, blocked)| *blocked)
            .count();
        assert!(
            during >= 5,
            "the guard fed only {during} times through the 70 s decision step"
        );
        assert!(
            disarmed.load(Ordering::SeqCst),
            "the guard disarmed at the end"
        );
    }

    #[test]
    fn a_panicking_guard_never_reports_the_commit() {
        // the feed thread dies on its first feed: no magic close
        // ever happens, and the decision must not answer Committed
        // — the commit already switched config.txt, so the
        // watchdog's reboot boots the new, healthy slot; only the
        // success report is withheld
        use std::sync::atomic::AtomicU64;
        let clock = Arc::new(AtomicU64::new(START));
        let disarmed = Arc::new(AtomicBool::new(false));

        struct PanickingGuard {
            clock: Arc<AtomicU64>,
            disarmed: Arc<AtomicBool>,
        }
        impl GuardWorld for PanickingGuard {
            fn uptime(&self) -> u64 {
                self.clock.load(Ordering::SeqCst)
            }
            fn arm(&self) {}
            fn feed(&self) {
                panic!("the feed thread died");
            }
            fn disarm(&self) {
                self.disarmed.store(true, Ordering::SeqCst);
            }
            fn log(&self, _line: &str) {}
            fn sleep(&self, _step: Duration) {
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        struct PanickedWorld {
            clock: Arc<AtomicU64>,
        }
        impl TrialWorld for PanickedWorld {
            fn uptime(&mut self) -> u64 {
                self.clock.load(Ordering::SeqCst)
            }
            fn mestro_pid(&mut self) -> Option<u32> {
                Some(100)
            }
            fn network_up(&mut self) -> bool {
                true
            }
            fn web_up(&mut self) -> bool {
                true
            }
            fn sleep(&mut self, step: Duration) {
                self.clock.fetch_add(step.as_secs(), Ordering::SeqCst);
            }
            fn log(&mut self, _line: &str) {}
            fn snapshot(&mut self) {}
            fn write_summary(&mut self, _result: &str) {}
            fn commit(&mut self) -> bool {
                true
            }
            fn reboot_rollback(&mut self) -> bool {
                false
            }
        }

        let guard = GuardThread::new(PanickingGuard {
            clock: clock.clone(),
            disarmed: disarmed.clone(),
        });
        let mut world = PanickedWorld { clock };
        assert_ne!(
            decide(&mut world, &guard),
            Outcome::Committed,
            "a dead guard is no successful commit"
        );
        assert!(!disarmed.load(Ordering::SeqCst), "no magic 'V' was written");
    }

    #[test]
    fn a_default_route_on_the_iface_counts() {
        // the kernel's own flat table format: hex addresses, the
        // default route is destination 00000000
        let table =
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                     eth0\t00000000\t0100A8C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
                     eth0\t00A8C0C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n\
                     wlan0\t00000000\t0100A8C0\t0003\t0\t0\t50\t00000000\t0\t0\t0\n";
        assert!(route_has_default(table, "eth0"));
        assert!(route_has_default(table, "wlan0"));
        assert!(!route_has_default(table, "usb0"));
    }

    #[test]
    fn a_subnet_route_is_not_a_default_route() {
        // the setup hotspot's table: wlan0 owns a subnet, no
        // default route anywhere
        let table =
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                     wlan0\t02A42A0A\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n";
        assert!(!route_has_default(table, "wlan0"));
        assert!(!route_has_default(table, "eth0"));
    }

    #[test]
    fn an_empty_table_has_no_default_route() {
        assert!(!route_has_default("", "wlan0"));
        let header =
            "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n";
        assert!(!route_has_default(header, "wlan0"));
    }
}
