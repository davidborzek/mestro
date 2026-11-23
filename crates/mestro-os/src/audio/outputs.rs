//! The audio output picker (feature `audio-output`): the ALSA
//! playback devices from [`ASOUND_DIR`], and switching squeezelite's
//! output through the `audio.output` setting. A switch that leaves
//! squeezelite dead is rolled back — the player must not end up silent
//! because someone picked a device that cannot be opened.

#![deny(clippy::indexing_slicing)]

use parking_lot::{Condvar, Mutex};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// the kernel's ALSA proc tree (`cards` plus `card<N>/pcm<D>p`): a
/// Linux fact, not an image choice
pub const ASOUND_DIR: &str = "/proc/asound";

/// one playback device the picker offers
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// the ALSA name squeezelite's `-o` takes: `hw:CARD=<id>`
    /// (`,DEV=<n>` for a card's further playback devices)
    pub id: String,
    /// the label to show
    pub name: String,
}

/// one line of `/proc/asound/cards`
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Card {
    pub(super) index: u32,
    pub(super) id: String,
    /// the kernel driver (`USB-Audio`, `vc4-hdmi`, …), empty when
    /// the line names none
    pub(super) driver: String,
    pub(super) name: String,
}

/// `/proc/asound/cards`: `<index> [<id>   ]: <driver> - <name>`,
/// each followed by an indented long-name line (skipped)
fn parse_cards(text: &str) -> Vec<Card> {
    text.lines().filter_map(parse_card_line).collect()
}

/// the cards under `asound_dir`, in card order
pub(super) fn list_cards(asound_dir: &Path) -> Vec<Card> {
    parse_cards(&std::fs::read_to_string(asound_dir.join("cards")).unwrap_or_default())
}

fn parse_card_line(line: &str) -> Option<Card> {
    let rest = line.trim_start();
    let (index, rest) = rest.split_once(char::is_whitespace)?;
    let index = index.parse().ok()?;
    let rest = rest.trim_start().strip_prefix('[')?;
    let (id, rest) = rest.split_once(']')?;
    let id = id.trim();
    if id.is_empty() {
        return None;
    }
    let rest = rest.trim_start().strip_prefix(':')?;
    let (driver, name) = rest.split_once(" - ").unwrap_or(("", rest));
    let name = name.trim();
    Some(Card {
        index,
        id: id.to_string(),
        driver: driver.trim().to_string(),
        name: if name.is_empty() { id } else { name }.to_string(),
    })
}

/// a playback pcm's label from its `info` file: the `name:` field,
/// else the `id:` one without ALSA's ` (*)` marker
fn parse_pcm_label(info: &str) -> Option<String> {
    let field = |wanted: &str| {
        info.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            (key.trim() == wanted).then(|| value.trim().trim_end_matches("(*)").trim().to_string())
        })
    };
    field("name")
        .filter(|name| !name.is_empty())
        .or_else(|| field("id").filter(|id| !id.is_empty()))
}

/// the playback devices under `asound_dir`: every card with at
/// least one `pcm<N>p` entry (capture-only cards are no output),
/// in card order. The first device of a card is plain
/// `hw:CARD=<id>`; further ones carry `,DEV=<n>` and their label
pub fn list_outputs(asound_dir: &Path) -> Vec<Output> {
    let mut outputs = Vec::new();
    for card in list_cards(asound_dir) {
        let card_dir = asound_dir.join(format!("card{}", card.index));
        let mut devices: Vec<u32> = std::fs::read_dir(&card_dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                name.strip_prefix("pcm")?.strip_suffix('p')?.parse().ok()
            })
            .collect();
        devices.sort_unstable();
        let several = devices.len() > 1;
        for device in devices {
            let label = || {
                std::fs::read_to_string(card_dir.join(format!("pcm{device}p/info")))
                    .ok()
                    .and_then(|info| parse_pcm_label(&info))
            };
            let id = if device == 0 {
                format!("hw:CARD={}", card.id)
            } else {
                format!("hw:CARD={},DEV={device}", card.id)
            };
            let name = match (several, label()) {
                (true, Some(label)) => format!("{} · {label}", card.name),
                _ => card.name.clone(),
            };
            outputs.push(Output { id, name });
        }
    }
    outputs
}

/// `hw:CARD=X,DEV=0` is `hw:CARD=X` to ALSA
fn same_device(a: &str, b: &str) -> bool {
    let canonical = |name: &str| name.trim().trim_end_matches(",DEV=0").to_string();
    canonical(a) == canonical(b)
}

/// the picker's list with the device in use (`current`, empty = none)
/// marked
pub fn outputs_with_current(
    current: &str,
    asound_dir: &Path,
) -> (Vec<(Output, bool)>, Option<String>) {
    let outputs = list_outputs(asound_dir)
        .into_iter()
        .map(|output| {
            let is_current = !current.is_empty() && same_device(current, &output.id);
            (output, is_current)
        })
        .collect();
    (
        outputs,
        Some(current.to_string()).filter(|current| !current.is_empty()),
    )
}

/// the capability: squeezelite's restart command exists (a desktop
/// without squeezelite has none). A bare program name is looked up in
/// `PATH`
pub fn available(restart_command: &str) -> bool {
    let Some(program) = restart_command.split_whitespace().next() else {
        return false;
    };
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .is_some_and(|dirs| std::env::split_paths(&dirs).any(|dir| dir.join(program).is_file()))
}

/// how squeezelite is restarted and watched — the seam the tests
/// replace
pub trait Squeezelite {
    /// run the restart command; an error is a non-zero exit or a
    /// command that cannot be started
    fn restart(&mut self) -> Result<(), String>;
    /// is a squeezelite process alive right now
    fn running(&mut self) -> bool;
    /// the arguments (without the program) the running squeezelite was
    /// started with, none when it is not running
    fn arguments(&mut self) -> Option<Vec<String>>;
    fn sleep(&mut self, duration: Duration);
}

/// one liveness poll per this
const WATCH_STEP: Duration = Duration::from_millis(500);
/// squeezelite must stay up this many polls after a restart: a
/// device it cannot open makes it exit within a fraction of a second
const WATCH_POLLS: u32 = 6;

/// is squeezelite still alive after the restart — polled over the
/// whole window, one miss is a failure
fn survives_startup(runner: &mut dyn Squeezelite) -> bool {
    for _ in 0..WATCH_POLLS {
        runner.sleep(WATCH_STEP);
        if !runner.running() {
            return false;
        }
    }
    true
}

/// env files with a switch in flight: the whole switch / restart /
/// verify / rollback transaction is one critical section per file,
/// and every background rewrite of the file waits for it
static SWITCHES_IN_FLIGHT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static SWITCH_DONE: Condvar = Condvar::new();

/// the switch slot of one env file, released on drop
pub(super) struct SwitchSlot(PathBuf);

impl SwitchSlot {
    pub(super) fn take(env_file: &Path) -> Result<Self, String> {
        let mut busy = SWITCHES_IN_FLIGHT.lock();
        if busy.iter().any(|path| path == env_file) {
            return Err("another switch is in progress".into());
        }
        busy.push(env_file.to_path_buf());
        Ok(Self(env_file.to_path_buf()))
    }

    /// block until the slot is free, for a background rewrite of the
    /// env file (the web request's `take` still fails fast)
    pub(super) fn wait(env_file: &Path) -> Self {
        let mut busy = SWITCHES_IN_FLIGHT.lock();
        while busy.iter().any(|path| path == env_file) {
            SWITCH_DONE.wait(&mut busy);
        }
        busy.push(env_file.to_path_buf());
        Self(env_file.to_path_buf())
    }
}

impl Drop for SwitchSlot {
    fn drop(&mut self) {
        SWITCHES_IN_FLIGHT.lock().retain(|path| *path != self.0);
        SWITCH_DONE.notify_all();
    }
}

/// where the output switch keeps the choice: the `audio.output`
/// setting (written through the normal settings path) and the env
/// file derived from it — the seam the tests replace
pub trait OutputSetting {
    /// the `audio.output` setting, empty = automatic
    fn get(&self) -> String;
    /// the output squeezelite plays on: the setting, else the derived one
    fn effective(&self) -> String;
    /// write the setting
    fn put(&mut self, value: &str) -> Result<(), String>;
    /// bring the env file in line with the settings and restart
    /// squeezelite, whatever changed
    fn apply(&mut self, runner: &mut dyn Squeezelite) -> Result<(), String>;
}

/// switch squeezelite to `output`: the id must be one of the
/// listed devices (anything else errors, nothing is touched; the one
/// in use is a no-op). The `audio.output` setting is set and
/// squeezelite restarts on the env file derived from it. If it is not running after the restart,
/// the previous setting value returns, squeezelite restarts again
/// and the error names the device that failed. One switch at a time
/// per `slot_key` (the env file): a second one fails at once with
/// "another switch is in progress"
pub fn set_output(
    slot_key: &Path,
    asound_dir: &Path,
    setting: &mut dyn OutputSetting,
    runner: &mut dyn Squeezelite,
    output: &str,
) -> Result<(), String> {
    let listed = list_outputs(asound_dir);
    if !listed.iter().any(|candidate| candidate.id == output) {
        return Err(format!("unknown audio output {output}"));
    }
    let _slot = SwitchSlot::take(slot_key)?;
    if same_device(&setting.effective(), output) {
        return Ok(());
    }
    let previous = setting.get();
    setting.put(output)?;
    let started = setting.apply(runner).and_then(|()| {
        survives_startup(runner)
            .then_some(())
            .ok_or_else(|| "squeezelite stopped right after the start".to_string())
    });
    let Err(reason) = started else {
        return Ok(());
    };
    setting.put(&previous).map_err(|e| {
        format!("{output} failed ({reason}) and the previous output could not be restored: {e}")
    })?;
    match setting.apply(runner) {
        Ok(()) => Err(format!(
            "squeezelite could not start on {output} ({reason}) — back on the previous output"
        )),
        Err(e) => Err(format!(
            "squeezelite could not start on {output} ({reason}) and the restart on the previous output failed: {e}"
        )),
    }
}

/// the real restart: the image's command, no shell, output
/// dropped (squeezelite is a daemon of that command — a pipe
/// would stay open as long as it runs)
pub struct SqueezeliteService {
    command: Vec<String>,
}

impl SqueezeliteService {
    /// `restart_command`: program and arguments, split on whitespace
    pub fn new(restart_command: &str) -> Self {
        Self {
            command: restart_command
                .split_whitespace()
                .map(String::from)
                .collect(),
        }
    }
}

impl Squeezelite for SqueezeliteService {
    fn restart(&mut self) -> Result<(), String> {
        let (program, args) = self
            .command
            .split_first()
            .ok_or("no squeezelite restart command")?;
        let status = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("{program}: {e}"))?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| format!("{program} exited with {status}"))
    }

    fn running(&mut self) -> bool {
        process_running(Path::new("/proc"), "squeezelite")
    }

    fn arguments(&mut self) -> Option<Vec<String>> {
        process_arguments(Path::new("/proc"), "squeezelite")
    }

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// the `/proc/<pid>` directory of a process whose `comm` is `name`
fn find_process(proc_dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(proc_dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_digit())
        })
        .find(|entry| {
            std::fs::read_to_string(entry.path().join("comm")).is_ok_and(|comm| comm.trim() == name)
        })
        .map(|entry| entry.path())
}

/// is there a process whose `comm` is `name` under `proc_dir`
fn process_running(proc_dir: &Path, name: &str) -> bool {
    find_process(proc_dir, name).is_some()
}

/// the command-line arguments (without the program) of a process whose
/// `comm` is `name` under `proc_dir`; none when there is none
fn process_arguments(proc_dir: &Path, name: &str) -> Option<Vec<String>> {
    let cmdline = std::fs::read(find_process(proc_dir, name)?.join("cmdline")).ok()?;
    Some(
        cmdline
            .split(|&b| b == 0)
            .skip(1)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;

    // a Pi 4 with a USB DAC: the headphone jack, both HDMI ports
    // and the DAC (cards as the kernel prints them)
    const PI_CARDS: &str = " 0 [Headphones     ]: bcm2835_headphonographic - bcm2835 Headphones\n\
                                               bcm2835 Headphones\n\
 1 [vc4hdmi0       ]: vc4-hdmi - vc4-hdmi-0\n\
                      vc4-hdmi-0\n\
 2 [vc4hdmi1       ]: vc4-hdmi - vc4-hdmi-1\n\
                      vc4-hdmi-1\n\
 3 [S3             ]: USB-Audio - Topping S3\n\
                      Topping Topping S3 at usb-0000:01:00.0-1.2, high speed\n\
 4 [Mic            ]: USB-Audio - USB Microphone\n\
                      Generic USB Microphone at usb-0000:01:00.0-1.3, full speed\n";

    // this machine's own /proc/asound/cards (a laptop codec card)
    const LAPTOP_CARDS: &str = " 2 [sofsoundwire   ]: sof-soundwire - sof-soundwire\n\
                      DellInc.-XPS14DA14260-080F9F\n";

    const PCM_INFO_JACK: &str = "card: 2\ndevice: 0\nsubdevice: 0\nstream: PLAYBACK\nid: Jack Out (*)\nname: \nsubname: subdevice #0\n";
    const PCM_INFO_SPEAKER: &str = "card: 2\ndevice: 2\nsubdevice: 0\nstream: PLAYBACK\nid: Speaker (*)\nname: \nsubname: subdevice #0\n";

    /// a fake /proc/asound: the cards file plus `card<N>/pcm<D><p|c>`
    /// directories (with an `info` file for the playback ones)
    fn asound(cards: &str, pcms: &[(u32, &str, Option<&str>)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cards"), cards).unwrap();
        for (card, pcm, info) in pcms {
            let path = dir.path().join(format!("card{card}")).join(pcm);
            std::fs::create_dir_all(&path).unwrap();
            if let Some(info) = info {
                std::fs::write(path.join("info"), info).unwrap();
            }
        }
        dir
    }

    fn pi_asound() -> tempfile::TempDir {
        asound(
            PI_CARDS,
            &[
                (
                    0,
                    "pcm0p",
                    Some("id: bcm2835 Headphones (*)\nname: bcm2835 Headphones\n"),
                ),
                (1, "pcm0p", Some("id: MAI PCM i2s-hifi-0 (*)\nname: \n")),
                (2, "pcm0p", Some("id: MAI PCM i2s-hifi-0 (*)\nname: \n")),
                (3, "pcm0p", Some("id: USB Audio (*)\nname: USB Audio\n")),
                (4, "pcm0c", None),
            ],
        )
    }

    #[test]
    fn the_cards_file_parses_ids_and_names_and_skips_the_long_names() {
        let cards = parse_cards(PI_CARDS);
        let summary: Vec<_> = cards
            .iter()
            .map(|c| (c.index, c.id.as_str(), c.name.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (0, "Headphones", "bcm2835 Headphones"),
                (1, "vc4hdmi0", "vc4-hdmi-0"),
                (2, "vc4hdmi1", "vc4-hdmi-1"),
                (3, "S3", "Topping S3"),
                (4, "Mic", "USB Microphone"),
            ]
        );
        assert_eq!(parse_cards(LAPTOP_CARDS).len(), 1);
        assert_eq!(
            parse_cards(LAPTOP_CARDS).first().unwrap().id,
            "sofsoundwire"
        );
        // no card at all ("--- no soundcards ---")
        assert!(parse_cards("--- no soundcards ---\n").is_empty());
        assert!(parse_cards("").is_empty());
    }

    #[test]
    fn a_card_carries_the_driver_its_line_names() {
        let drivers: Vec<_> = parse_cards(PI_CARDS)
            .into_iter()
            .map(|card| card.driver)
            .collect();
        assert_eq!(
            drivers,
            [
                "bcm2835_headphonographic",
                "vc4-hdmi",
                "vc4-hdmi",
                "USB-Audio",
                "USB-Audio"
            ]
        );
        // a line without a driver part has none
        let bare = parse_cards(" 0 [Dummy ]: Just a name\n");
        assert_eq!(bare.first().map(|c| c.driver.as_str()), Some(""));
        assert_eq!(bare.first().map(|c| c.name.as_str()), Some("Just a name"));
    }

    #[test]
    fn a_pi_lists_headphones_hdmi_and_the_usb_dac_but_not_the_microphone() {
        let dir = pi_asound();
        let outputs = list_outputs(dir.path());
        let listed: Vec<_> = outputs
            .iter()
            .map(|o| (o.id.as_str(), o.name.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("hw:CARD=Headphones", "bcm2835 Headphones"),
                ("hw:CARD=vc4hdmi0", "vc4-hdmi-0"),
                ("hw:CARD=vc4hdmi1", "vc4-hdmi-1"),
                ("hw:CARD=S3", "Topping S3"),
            ]
        );
    }

    #[test]
    fn a_card_with_several_playback_devices_lists_each_with_its_label() {
        let dir = asound(
            LAPTOP_CARDS,
            &[
                (2, "pcm0p", Some(PCM_INFO_JACK)),
                (2, "pcm2p", Some(PCM_INFO_SPEAKER)),
                (2, "pcm31p", Some("id: Deepbuffer Jack Out (*)\nname: \n")),
                (2, "pcm1c", None),
            ],
        );
        let outputs = list_outputs(dir.path());
        let listed: Vec<_> = outputs
            .iter()
            .map(|o| (o.id.as_str(), o.name.as_str()))
            .collect();
        assert_eq!(
            listed,
            [
                ("hw:CARD=sofsoundwire", "sof-soundwire · Jack Out"),
                ("hw:CARD=sofsoundwire,DEV=2", "sof-soundwire · Speaker"),
                (
                    "hw:CARD=sofsoundwire,DEV=31",
                    "sof-soundwire · Deepbuffer Jack Out"
                ),
            ]
        );
    }

    #[test]
    fn no_asound_tree_means_no_outputs() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_outputs(&dir.path().join("missing")).is_empty());
        assert!(list_outputs(dir.path()).is_empty());
    }

    #[test]
    fn the_current_output_is_the_one_the_setting_names() {
        let asound = pi_asound();
        let (outputs, current) = outputs_with_current("hw:CARD=S3,DEV=0", asound.path());
        assert_eq!(current.as_deref(), Some("hw:CARD=S3,DEV=0"));
        let marked: Vec<_> = outputs
            .iter()
            .filter(|(_, c)| *c)
            .map(|(o, _)| o.id.as_str())
            .collect();
        assert_eq!(marked, ["hw:CARD=S3"], "DEV=0 is the plain card");
        // an output that is not plugged in marks nothing
        assert!(outputs_with_current("hw:CARD=DAC", asound.path())
            .0
            .iter()
            .all(|(_, c)| !c));
        // nothing in use: nothing marked, no current
        let (outputs, current) = outputs_with_current("", asound.path());
        assert_eq!(current, None);
        assert!(outputs.iter().all(|(_, c)| !c));
    }

    #[test]
    fn the_capability_follows_the_restart_command() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("S60squeezelite");
        let command = format!("{} restart", script.display());
        assert!(!available(&command));
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        assert!(available(&command));
        assert!(!available(""));
        // a bare program name is looked up in PATH
        assert!(available("sh -c true"));
        assert!(!available("no-such-program-anywhere restart"));
    }

    /// the world a switch acts on: the `audio.output` setting's value
    type Shared = Arc<Mutex<String>>;

    /// a squeezelite stand-in: remembers which output the setting had
    /// at every restart; the listed devices "cannot be opened"
    struct Fake {
        setting: Shared,
        broken: Vec<&'static str>,
        restarts: Vec<String>,
        restart_fails: bool,
        slept: Duration,
    }

    impl Fake {
        fn new(setting: &Shared, broken: &[&'static str]) -> Self {
            Self {
                setting: setting.clone(),
                broken: broken.to_vec(),
                restarts: Vec::new(),
                restart_fails: false,
                slept: Duration::ZERO,
            }
        }
    }

    impl Squeezelite for Fake {
        fn restart(&mut self) -> Result<(), String> {
            self.restarts.push(self.setting.lock().clone());
            if self.restart_fails {
                return Err("init script exited with 1".into());
            }
            Ok(())
        }
        fn running(&mut self) -> bool {
            !self.broken.contains(&self.setting.lock().as_str())
        }
        fn arguments(&mut self) -> Option<Vec<String>> {
            self.running().then(Vec::new)
        }
        fn sleep(&mut self, duration: Duration) {
            self.slept += duration;
        }
    }

    /// the setting's stand-in: its value is what the env file would
    /// be derived from
    struct FakeSetting {
        value: Shared,
        writes: Vec<String>,
        put_fails: bool,
    }

    impl FakeSetting {
        fn new(value: &Shared) -> Self {
            Self {
                value: value.clone(),
                writes: Vec::new(),
                put_fails: false,
            }
        }
    }

    impl OutputSetting for FakeSetting {
        fn get(&self) -> String {
            self.value.lock().clone()
        }
        fn effective(&self) -> String {
            self.value.lock().clone()
        }
        fn put(&mut self, value: &str) -> Result<(), String> {
            if self.put_fails {
                return Err("settings are read-only".into());
            }
            self.writes.push(value.to_string());
            *self.value.lock() = value.to_string();
            Ok(())
        }
        fn apply(&mut self, runner: &mut dyn Squeezelite) -> Result<(), String> {
            runner.restart()
        }
    }

    /// a switch's surroundings: the asound tree, the setting's value
    struct Setup {
        _asound: tempfile::TempDir,
        asound_dir: PathBuf,
        slot_key: PathBuf,
        setting: Shared,
    }

    fn setup() -> Setup {
        let asound = pi_asound();
        Setup {
            asound_dir: asound.path().to_path_buf(),
            slot_key: asound.path().join("squeezelite.env"),
            _asound: asound,
            setting: Arc::new(Mutex::new("hw:CARD=Headphones".to_string())),
        }
    }

    fn set_output(
        config: &Setup,
        setting: &mut FakeSetting,
        runner: &mut dyn Squeezelite,
        output: &str,
    ) -> Result<(), String> {
        super::set_output(
            &config.slot_key,
            &config.asound_dir,
            setting,
            runner,
            output,
        )
    }

    #[test]
    fn a_switch_sets_the_setting_and_restarts_once() {
        let config = setup();
        let mut fake = Fake::new(&config.setting, &[]);
        let mut setting = FakeSetting::new(&config.setting);
        set_output(&config, &mut setting, &mut fake, "hw:CARD=S3").unwrap();
        assert_eq!(config.setting.lock().as_str(), "hw:CARD=S3");
        assert_eq!(setting.writes, ["hw:CARD=S3"]);
        assert_eq!(fake.restarts, ["hw:CARD=S3"]);
        assert!(
            fake.slept >= WATCH_STEP * WATCH_POLLS,
            "the watch window ran"
        );
    }

    #[test]
    fn an_unknown_output_is_refused_with_nothing_touched() {
        let config = setup();
        let mut fake = Fake::new(&config.setting, &[]);
        let mut setting = FakeSetting::new(&config.setting);
        for bad in ["hw:CARD=DAC", "../../etc/passwd", "hw:CARD=S3\nEXTRA=x", ""] {
            let err = set_output(&config, &mut setting, &mut fake, bad).unwrap_err();
            assert!(err.contains("unknown audio output"), "{bad:?}: {err}");
        }
        assert!(setting.writes.is_empty());
        assert_eq!(config.setting.lock().as_str(), "hw:CARD=Headphones");
        assert!(fake.restarts.is_empty(), "squeezelite was not touched");
    }

    #[test]
    fn choosing_the_output_of_the_setting_changes_nothing() {
        let config = setup();
        let mut fake = Fake::new(&config.setting, &[]);
        let mut setting = FakeSetting::new(&config.setting);
        set_output(&config, &mut setting, &mut fake, "hw:CARD=Headphones").unwrap();
        assert!(setting.writes.is_empty());
        assert!(fake.restarts.is_empty());
        // the same card by its other name
        *config.setting.lock() = "hw:CARD=S3,DEV=0".into();
        set_output(&config, &mut setting, &mut fake, "hw:CARD=S3").unwrap();
        assert!(setting.writes.is_empty());
        assert!(fake.restarts.is_empty());
    }

    #[test]
    fn a_device_squeezelite_cannot_open_is_rolled_back_and_named() {
        let config = setup();
        let mut fake = Fake::new(&config.setting, &["hw:CARD=S3"]);
        let mut setting = FakeSetting::new(&config.setting);
        let err = set_output(&config, &mut setting, &mut fake, "hw:CARD=S3").unwrap_err();
        assert!(err.contains("hw:CARD=S3"), "{err}");
        assert!(err.contains("previous output"), "{err}");
        // the setting is the old value again
        assert_eq!(config.setting.lock().as_str(), "hw:CARD=Headphones");
        assert_eq!(setting.writes, ["hw:CARD=S3", "hw:CARD=Headphones"]);
        // restarted on the new output, then again on the old one
        assert_eq!(fake.restarts, ["hw:CARD=S3", "hw:CARD=Headphones"]);
    }

    #[test]
    fn a_rollback_restores_an_automatic_setting_as_empty() {
        let config = setup();
        *config.setting.lock() = String::new();
        let mut fake = Fake::new(&config.setting, &["hw:CARD=S3"]);
        let mut setting = FakeSetting::new(&config.setting);
        assert!(set_output(&config, &mut setting, &mut fake, "hw:CARD=S3").is_err());
        assert_eq!(config.setting.lock().as_str(), "");
        assert_eq!(fake.restarts, ["hw:CARD=S3", ""]);
    }

    #[test]
    fn a_failing_init_script_is_rolled_back_too() {
        let config = setup();
        let mut fake = Fake::new(&config.setting, &[]);
        fake.restart_fails = true;
        let mut setting = FakeSetting::new(&config.setting);
        let err = set_output(&config, &mut setting, &mut fake, "hw:CARD=S3").unwrap_err();
        assert!(err.contains("hw:CARD=S3"), "{err}");
        assert!(
            err.contains("restart on the previous output failed"),
            "{err}"
        );
        assert_eq!(config.setting.lock().as_str(), "hw:CARD=Headphones");
        assert_eq!(fake.restarts.len(), 2);
    }

    #[test]
    fn a_setting_that_cannot_be_written_fails_before_squeezelite_is_touched() {
        let config = setup();
        let mut fake = Fake::new(&config.setting, &[]);
        let mut setting = FakeSetting::new(&config.setting);
        setting.put_fails = true;
        let err = set_output(&config, &mut setting, &mut fake, "hw:CARD=S3").unwrap_err();
        assert!(err.contains("read-only"), "{err}");
        assert!(fake.restarts.is_empty());
    }

    /// a squeezelite stand-in that parks inside the restart until released
    struct Parked {
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    }

    impl Squeezelite for Parked {
        fn restart(&mut self) -> Result<(), String> {
            self.entered.send(()).map_err(|e| e.to_string())?;
            self.release.recv().map_err(|e| e.to_string())
        }
        fn running(&mut self) -> bool {
            true
        }
        fn arguments(&mut self) -> Option<Vec<String>> {
            Some(Vec::new())
        }
        fn sleep(&mut self, _duration: Duration) {}
    }

    #[test]
    fn a_second_switch_fails_fast_while_one_is_in_progress() {
        let config = setup();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                let mut parked = Parked {
                    entered: entered_tx,
                    release: release_rx,
                };
                let mut setting = FakeSetting::new(&config.setting);
                set_output(&config, &mut setting, &mut parked, "hw:CARD=S3")
            });
            // the first switch is inside its restart: the setting is written
            entered_rx.recv().unwrap();
            let mut fake = Fake::new(&config.setting, &[]);
            let mut setting = FakeSetting::new(&config.setting);
            let second = set_output(&config, &mut setting, &mut fake, "hw:CARD=vc4hdmi0");
            let in_flight = config.setting.lock().clone();
            // release before asserting: a failed assert must not hang the scope
            release_tx.send(()).unwrap();
            first.join().unwrap().unwrap();
            let err = second.unwrap_err();
            assert!(err.contains("another switch is in progress"), "{err}");
            assert!(fake.restarts.is_empty(), "the busy request touched nothing");
            assert!(setting.writes.is_empty());
            assert_eq!(
                in_flight, "hw:CARD=S3",
                "the running switch's value is intact"
            );
        });
        assert_eq!(config.setting.lock().as_str(), "hw:CARD=S3");
        // the lock is free again — also after a rolled-back switch
        let mut fake = Fake::new(&config.setting, &["hw:CARD=vc4hdmi0"]);
        let mut setting = FakeSetting::new(&config.setting);
        assert!(set_output(&config, &mut setting, &mut fake, "hw:CARD=vc4hdmi0").is_err());
        let mut fake = Fake::new(&config.setting, &[]);
        set_output(&config, &mut setting, &mut fake, "hw:CARD=Headphones").unwrap();
    }

    #[test]
    fn waiting_for_the_switch_slot_blocks_until_it_is_released() {
        let env = Path::new("/waiting-for-the-switch-slot/squeezelite.env");
        let held = SwitchSlot::take(env).unwrap();
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| drop(SwitchSlot::wait(env)));
            std::thread::sleep(Duration::from_millis(50));
            assert!(!waiter.is_finished(), "the slot is still held");
            drop(held);
            waiter.join().unwrap();
        });
        assert!(SwitchSlot::take(env).is_ok(), "released again");
    }

    #[test]
    fn switches_of_different_slots_do_not_block_each_other() {
        let config_a = setup();
        let config_b = setup();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                let mut parked = Parked {
                    entered: entered_tx,
                    release: release_rx,
                };
                let mut setting = FakeSetting::new(&config_a.setting);
                set_output(&config_a, &mut setting, &mut parked, "hw:CARD=S3")
            });
            entered_rx.recv().unwrap();
            let mut fake = Fake::new(&config_b.setting, &[]);
            let mut setting = FakeSetting::new(&config_b.setting);
            set_output(&config_b, &mut setting, &mut fake, "hw:CARD=S3").unwrap();
            release_tx.send(()).unwrap();
            first.join().unwrap().unwrap();
        });
    }

    #[test]
    fn the_process_probe_matches_the_comm_name_only() {
        let proc = tempfile::tempdir().unwrap();
        for (pid, comm) in [
            ("101", "squeezelite\n"),
            ("102", "mestro\n"),
            ("self", "squeezelite\n"),
        ] {
            std::fs::create_dir_all(proc.path().join(pid)).unwrap();
            std::fs::write(proc.path().join(pid).join("comm"), comm).unwrap();
        }
        assert!(process_running(proc.path(), "squeezelite"));
        assert!(!process_running(proc.path(), "squeeze"));
        std::fs::remove_dir_all(proc.path().join("101")).unwrap();
        assert!(
            !process_running(proc.path(), "squeezelite"),
            "non-numeric entries are no processes"
        );
    }

    #[test]
    fn the_process_arguments_are_the_cmdline_without_the_program() {
        let proc = tempfile::tempdir().unwrap();
        for (pid, comm, cmdline) in [
            (
                "101",
                "squeezelite\n",
                &b"/usr/bin/squeezelite\0-v\0-o\0hw:CARD=S3\0-n\0Hi Fi\0"[..],
            ),
            ("102", "mestro\n", &b"/usr/bin/mestro\0--kiosk\0"[..]),
        ] {
            std::fs::create_dir_all(proc.path().join(pid)).unwrap();
            std::fs::write(proc.path().join(pid).join("comm"), comm).unwrap();
            std::fs::write(proc.path().join(pid).join("cmdline"), cmdline).unwrap();
        }
        assert_eq!(
            process_arguments(proc.path(), "squeezelite").unwrap(),
            ["-v", "-o", "hw:CARD=S3", "-n", "Hi Fi"]
        );
        std::fs::write(proc.path().join("101/cmdline"), b"/usr/bin/squeezelite\0").unwrap();
        assert_eq!(
            process_arguments(proc.path(), "squeezelite").unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(process_arguments(proc.path(), "nothing"), None);
        std::fs::remove_dir_all(proc.path().join("101")).unwrap();
        assert_eq!(process_arguments(proc.path(), "squeezelite"), None);
    }

    #[test]
    fn the_real_service_reports_a_failing_command() {
        let service = SqueezeliteService::new;
        assert!(service("true").restart().is_ok());
        assert!(service("false").restart().unwrap_err().contains("exited"));
        assert!(service("/nonexistent/init restart").restart().is_err());
        assert!(service("").restart().is_err());
    }
}
