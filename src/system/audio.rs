//! The audio output picker (feature `audio-output`): the ALSA
//! playback devices from [`ASOUND_DIR`], and switching squeezelite's
//! `OUTPUT` in its env file. A switch that leaves squeezelite dead
//! is rolled back — the player must not end up silent because
//! someone picked a device that cannot be opened.

#![deny(clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
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
struct Card {
    index: u32,
    id: String,
    name: String,
}

/// `/proc/asound/cards`: `<index> [<id>   ]: <driver> - <name>`,
/// each followed by an indented long-name line (skipped)
fn parse_cards(text: &str) -> Vec<Card> {
    text.lines().filter_map(parse_card_line).collect()
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
    let name = rest
        .split_once(" - ")
        .map_or(rest, |(_driver, name)| name)
        .trim();
    Some(Card {
        index,
        id: id.to_string(),
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
    let cards = std::fs::read_to_string(asound_dir.join("cards")).unwrap_or_default();
    let mut outputs = Vec::new();
    for card in parse_cards(&cards) {
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

/// the `OUTPUT` the env file sets, None when it is not there
pub fn current_output(env_file: &Path) -> Option<String> {
    super::envfile::get_key_in_file(env_file, "OUTPUT")
}

/// the picker's list with the device in use marked
pub fn outputs_with_current(
    env_file: &Path,
    asound_dir: &Path,
) -> (Vec<(Output, bool)>, Option<String>) {
    let current = current_output(env_file);
    let outputs = list_outputs(asound_dir)
        .into_iter()
        .map(|output| {
            let is_current = current
                .as_deref()
                .is_some_and(|current| same_device(current, &output.id));
            (output, is_current)
        })
        .collect();
    (outputs, current)
}

/// the capability: the env file exists (a desktop without
/// squeezelite has none)
pub fn available(env_file: &Path) -> bool {
    env_file.is_file()
}

/// how squeezelite is restarted and watched — the seam the tests
/// replace
pub trait Squeezelite {
    /// run the restart command; an error is a non-zero exit or a
    /// command that cannot be started
    fn restart(&mut self) -> Result<(), String>;
    /// is a squeezelite process alive right now
    fn running(&mut self) -> bool;
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
/// verify / rollback transaction is one critical section per file
static SWITCHES_IN_FLIGHT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// the switch slot of one env file, released on drop
struct SwitchSlot(PathBuf);

impl SwitchSlot {
    fn take(env_file: &Path) -> Result<Self, String> {
        let mut busy = crate::locks::lock(&SWITCHES_IN_FLIGHT);
        if busy.iter().any(|path| path == env_file) {
            return Err("another switch is in progress".into());
        }
        busy.push(env_file.to_path_buf());
        Ok(Self(env_file.to_path_buf()))
    }
}

impl Drop for SwitchSlot {
    fn drop(&mut self) {
        crate::locks::lock(&SWITCHES_IN_FLIGHT).retain(|path| *path != self.0);
    }
}

/// switch squeezelite to `output`: the id must be one of the
/// listed devices (anything else errors with the file untouched),
/// only the `OUTPUT` line changes, squeezelite restarts. If it is
/// not running after the restart, the previous file is restored
/// byte for byte, squeezelite restarts again and the error names
/// the device that failed. One switch per env file at a time: a
/// second one fails at once with "another switch is in progress"
pub fn set_output(
    env_file: &Path,
    asound_dir: &Path,
    runner: &mut dyn Squeezelite,
    output: &str,
) -> Result<(), String> {
    let listed = list_outputs(asound_dir);
    if !listed.iter().any(|candidate| candidate.id == output) {
        return Err(format!("unknown audio output {output}"));
    }
    let _slot = SwitchSlot::take(env_file)?;
    if current_output(env_file).is_some_and(|current| same_device(&current, output)) {
        return Ok(());
    }
    let before = std::fs::read_to_string(env_file)
        .map_err(|e| format!("read {}: {e}", env_file.display()))?;
    super::envfile::set_key_in_file(env_file, "OUTPUT", output)?;
    let started = runner.restart().and_then(|()| {
        survives_startup(runner)
            .then_some(())
            .ok_or_else(|| "squeezelite stopped right after the start".to_string())
    });
    let Err(reason) = started else {
        return Ok(());
    };
    crate::atomic_file::write_atomic(env_file, &before).map_err(|e| {
        format!("{output} failed ({reason}) and the previous output could not be restored: {e}")
    })?;
    match runner.restart() {
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

    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// is there a process whose `comm` is `name` under `proc_dir`
fn process_running(proc_dir: &Path, name: &str) -> bool {
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
        .any(|entry| {
            std::fs::read_to_string(entry.path().join("comm")).is_ok_and(|comm| comm.trim() == name)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_current_output_is_the_one_the_env_file_names() {
        let asound = pi_asound();
        let env = tempfile::tempdir().unwrap();
        let env_file = env.path().join("squeezelite.env");
        std::fs::write(&env_file, "OUTPUT=\"hw:CARD=S3,DEV=0\"\nNAME=x\n").unwrap();
        let (outputs, current) = outputs_with_current(&env_file, asound.path());
        assert_eq!(current.as_deref(), Some("hw:CARD=S3,DEV=0"));
        let marked: Vec<_> = outputs
            .iter()
            .filter(|(_, c)| *c)
            .map(|(o, _)| o.id.as_str())
            .collect();
        assert_eq!(marked, ["hw:CARD=S3"], "DEV=0 is the plain card");
        // an output that is not plugged in marks nothing
        std::fs::write(&env_file, "OUTPUT=hw:CARD=DAC\n").unwrap();
        assert!(outputs_with_current(&env_file, asound.path())
            .0
            .iter()
            .all(|(_, c)| !c));
    }

    #[test]
    fn the_capability_follows_the_env_file() {
        let dir = tempfile::tempdir().unwrap();
        let env_file = dir.path().join("squeezelite.env");
        assert!(!available(&env_file));
        std::fs::write(&env_file, "OUTPUT=x\n").unwrap();
        assert!(available(&env_file));
    }

    /// a squeezelite stand-in: remembers which OUTPUT the env file
    /// had at every restart; the listed devices "cannot be opened"
    struct Fake {
        env: std::path::PathBuf,
        broken: Vec<&'static str>,
        restarts: Vec<Option<String>>,
        restart_fails: bool,
        slept: Duration,
    }

    impl Fake {
        fn new(config: &Setup, broken: &[&'static str]) -> Self {
            Self {
                env: config.env_file.clone(),
                broken: broken.to_vec(),
                restarts: Vec::new(),
                restart_fails: false,
                slept: Duration::ZERO,
            }
        }
        fn output(&self) -> Option<String> {
            current_output(&self.env)
        }
    }

    impl Squeezelite for Fake {
        fn restart(&mut self) -> Result<(), String> {
            self.restarts.push(self.output());
            if self.restart_fails {
                return Err("init script exited with 1".into());
            }
            Ok(())
        }
        fn running(&mut self) -> bool {
            let output = self.output().unwrap_or_default();
            !self.broken.contains(&output.as_str())
        }
        fn sleep(&mut self, duration: Duration) {
            self.slept += duration;
        }
    }

    const ENV: &str =
        "# squeezelite env\nOUTPUT=hw:CARD=Headphones\n# host\nSERVER=\nNAME=mestro\n";

    /// the two paths a switch works on, in temp dirs
    struct Setup {
        env_file: PathBuf,
        asound_dir: PathBuf,
    }

    fn set_output(
        config: &Setup,
        runner: &mut dyn Squeezelite,
        output: &str,
    ) -> Result<(), String> {
        super::set_output(&config.env_file, &config.asound_dir, runner, output)
    }

    fn setup() -> (tempfile::TempDir, tempfile::TempDir, Setup) {
        let asound = pi_asound();
        let env = tempfile::tempdir().unwrap();
        let config = Setup {
            env_file: env.path().join("squeezelite.env"),
            asound_dir: asound.path().to_path_buf(),
        };
        std::fs::write(&config.env_file, ENV).unwrap();
        (asound, env, config)
    }

    #[test]
    fn a_switch_rewrites_only_the_output_line_and_restarts_once() {
        let (_asound, _env, config) = setup();
        let mut fake = Fake::new(&config, &[]);
        set_output(&config, &mut fake, "hw:CARD=S3").unwrap();
        assert_eq!(
            std::fs::read_to_string(&config.env_file).unwrap(),
            ENV.replace("hw:CARD=Headphones", "hw:CARD=S3")
        );
        assert_eq!(fake.restarts, [Some("hw:CARD=S3".to_string())]);
        assert!(
            fake.slept >= WATCH_STEP * WATCH_POLLS,
            "the watch window ran"
        );
    }

    #[test]
    fn an_unknown_output_is_refused_with_the_file_untouched() {
        let (_asound, _env, config) = setup();
        let mut fake = Fake::new(&config, &[]);
        for bad in ["hw:CARD=DAC", "../../etc/passwd", "hw:CARD=S3\nEXTRA=x", ""] {
            let err = set_output(&config, &mut fake, bad).unwrap_err();
            assert!(err.contains("unknown audio output"), "{bad:?}: {err}");
        }
        assert_eq!(std::fs::read_to_string(&config.env_file).unwrap(), ENV);
        assert!(fake.restarts.is_empty(), "squeezelite was not touched");
    }

    #[test]
    fn choosing_the_running_output_changes_nothing() {
        let (_asound, _env, config) = setup();
        let mut fake = Fake::new(&config, &[]);
        set_output(&config, &mut fake, "hw:CARD=Headphones").unwrap();
        assert_eq!(std::fs::read_to_string(&config.env_file).unwrap(), ENV);
        assert!(fake.restarts.is_empty());
    }

    #[test]
    fn a_device_squeezelite_cannot_open_is_rolled_back_and_named() {
        let (_asound, _env, config) = setup();
        let mut fake = Fake::new(&config, &["hw:CARD=S3"]);
        let err = set_output(&config, &mut fake, "hw:CARD=S3").unwrap_err();
        assert!(err.contains("hw:CARD=S3"), "{err}");
        assert!(err.contains("previous output"), "{err}");
        // the file is exactly the old one again, comments included
        assert_eq!(std::fs::read_to_string(&config.env_file).unwrap(), ENV);
        // restarted on the new output, then again on the old one
        assert_eq!(
            fake.restarts,
            [
                Some("hw:CARD=S3".to_string()),
                Some("hw:CARD=Headphones".to_string())
            ]
        );
    }

    #[test]
    fn a_failing_init_script_is_rolled_back_too() {
        let (_asound, _env, config) = setup();
        let mut fake = Fake::new(&config, &[]);
        fake.restart_fails = true;
        let err = set_output(&config, &mut fake, "hw:CARD=S3").unwrap_err();
        assert!(err.contains("hw:CARD=S3"), "{err}");
        assert!(
            err.contains("restart on the previous output failed"),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(&config.env_file).unwrap(), ENV);
        assert_eq!(fake.restarts.len(), 2);
    }

    #[test]
    fn a_missing_output_key_is_appended_and_removed_again_on_rollback() {
        let (_asound, _env, config) = setup();
        let bare = "NAME=mestro\n";
        std::fs::write(&config.env_file, bare).unwrap();
        let mut fake = Fake::new(&config, &["hw:CARD=S3"]);
        assert!(set_output(&config, &mut fake, "hw:CARD=S3").is_err());
        assert_eq!(std::fs::read_to_string(&config.env_file).unwrap(), bare);
        let mut fake = Fake::new(&config, &[]);
        set_output(&config, &mut fake, "hw:CARD=S3").unwrap();
        assert_eq!(
            std::fs::read_to_string(&config.env_file).unwrap(),
            "NAME=mestro\nOUTPUT=hw:CARD=S3\n"
        );
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
        fn sleep(&mut self, _duration: Duration) {}
    }

    #[test]
    fn a_second_switch_fails_fast_while_one_is_in_progress() {
        let (_asound, _env, config) = setup();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                let mut parked = Parked {
                    entered: entered_tx,
                    release: release_rx,
                };
                set_output(&config, &mut parked, "hw:CARD=S3")
            });
            // the first switch is inside its restart: the file is written
            entered_rx.recv().unwrap();
            let mut fake = Fake::new(&config, &[]);
            let second = set_output(&config, &mut fake, "hw:CARD=vc4hdmi0");
            let on_disk = current_output(&config.env_file);
            // release before asserting: a failed assert must not hang the scope
            release_tx.send(()).unwrap();
            first.join().unwrap().unwrap();
            let err = second.unwrap_err();
            assert!(err.contains("another switch is in progress"), "{err}");
            assert!(fake.restarts.is_empty(), "the busy request touched nothing");
            assert_eq!(
                on_disk.as_deref(),
                Some("hw:CARD=S3"),
                "the running switch's file is intact"
            );
        });
        assert_eq!(
            std::fs::read_to_string(&config.env_file).unwrap(),
            ENV.replace("hw:CARD=Headphones", "hw:CARD=S3")
        );
        // the lock is free again — also after a rolled-back switch
        let mut fake = Fake::new(&config, &["hw:CARD=vc4hdmi0"]);
        assert!(set_output(&config, &mut fake, "hw:CARD=vc4hdmi0").is_err());
        let mut fake = Fake::new(&config, &[]);
        set_output(&config, &mut fake, "hw:CARD=Headphones").unwrap();
    }

    #[test]
    fn switches_of_different_env_files_do_not_block_each_other() {
        let (_asound_a, _env_a, config_a) = setup();
        let (_asound_b, _env_b, config_b) = setup();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                let mut parked = Parked {
                    entered: entered_tx,
                    release: release_rx,
                };
                set_output(&config_a, &mut parked, "hw:CARD=S3")
            });
            entered_rx.recv().unwrap();
            let mut fake = Fake::new(&config_b, &[]);
            set_output(&config_b, &mut fake, "hw:CARD=S3").unwrap();
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
    fn the_real_service_reports_a_failing_command() {
        let service = SqueezeliteService::new;
        assert!(service("true").restart().is_ok());
        assert!(service("false").restart().unwrap_err().contains("exited"));
        assert!(service("/nonexistent/init restart").restart().is_err());
        assert!(service("").restart().is_err());
    }
}
