//! squeezelite's env file (feature `audio-output`): mestro owns it. The
//! values come from the `audio.*` settings, an empty one falls back
//! to what mestro derives itself — the output device (a USB DAC, else
//! a HAT), the player name (`mestro-XXXX`) and the Music Assistant
//! host (of `music-assistant.url`). The file is written whole, only
//! when its content changes, and is the only file the init script
//! sources; nobody else writes it.

#![deny(clippy::indexing_slicing)]

use super::audio::{self, Card, Output, Squeezelite, SwitchSlot};
use crate::config::Audio;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// one wait step while a card may still be enumerating
const SETTLE_STEP: Duration = Duration::from_secs(1);
/// a startup run waits this many steps at most (~10 s): a slow USB
/// DAC is not in `/proc/asound` yet when mestro starts
const SETTLE_POLLS: u32 = 10;

/// one wait step while squeezelite is not up yet
const START_STEP: Duration = Duration::from_millis(500);
/// a startup run waits this many steps at most (40 s) for squeezelite:
/// its init script starts after mestro and waits for the network itself,
/// up to 30 s
const START_POLLS: u32 = 80;

/// what the file starts with
const HEADER: &str =
    "# written by mestro from its audio settings — do not edit (Settings → Audio)\n";

/// what a run composes the file from: the newest settings, read when
/// the run's turn comes, so an older change can never overwrite a
/// newer one
#[derive(Debug, Clone, Default)]
struct Inputs {
    audio: Audio,
    ma_url: String,
}

static INPUTS: Mutex<Option<Inputs>> = Mutex::new(None);

/// what the startup run derived (the settings may override it)
static DERIVED: Mutex<Option<Derived>> = Mutex::new(None);

/// one run at a time: each compares with the file the previous left
static RUNS: Mutex<()> = Mutex::new(());

/// the files a run works on
struct Paths<'a> {
    /// mestro's env file, written whole; the output switch's slot is
    /// keyed by it
    env_file: &'a Path,
    asound_dir: &'a Path,
}

/// what mestro derives without a setting: decided once, at startup (a
/// card plugged in later is the user's pick in the web app)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Derived {
    output: String,
    name: String,
}

/// the device a startup run derives `OUTPUT` from
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pick {
    id: String,
    usb: bool,
}

/// the file's values
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Env {
    output: String,
    name: String,
    server: String,
    extra: String,
}

/// who asks for the run
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// a settings change or the startup: waits for an output switch
    /// in flight (and composes only after it), restarts squeezelite
    /// only when the file changed and it runs
    Background,
    /// the output switch (the caller holds the switch slot): restarts
    /// squeezelite whatever it did before, and reports a failing restart
    Switch,
}

/// `mestro-XXXX`, none without a suffix
fn default_name(suffix: &str) -> String {
    if suffix.is_empty() {
        String::new()
    } else {
        format!("mestro-{suffix}")
    }
}

/// the host of a Music Assistant url (`http://host:8095`,
/// `ws://host:8095/ws`, a bare `host:8095`): squeezelite's `-s` takes
/// the host and finds MA's slimproto port (3483, its default) itself.
/// None for what squeezelite cannot dial (an IPv6 literal) or garbage
fn server_host(ma_url: &str) -> Option<String> {
    let url = ma_url.trim();
    let rest = url.split_once("://").map_or(url, |(_scheme, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host_port = authority.rsplit('@').next()?;
    let host = host_port.split(':').next()?;
    let valid = !host.is_empty()
        && !host_port.starts_with('[')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    valid.then(|| host.to_string())
}

fn is_usb(card: &Card) -> bool {
    card.driver.eq_ignore_ascii_case("USB-Audio")
}

/// the Pi's own jack and HDMI ports: never a default, squeezelite's
/// own default does as well
fn is_builtin(card: &Card) -> bool {
    let driver = card.driver.to_ascii_lowercase();
    driver.starts_with("bcm2835") || driver.starts_with("vc4-hdmi")
}

/// the card's first playback device (`hw:CARD=<id>`, `,DEV=<n>`
/// when it has none on device 0)
fn first_output(card: &Card, outputs: &[Output]) -> Option<String> {
    let plain = format!("hw:CARD={}", card.id);
    let further = format!("{plain},");
    outputs
        .iter()
        .find(|output| output.id == plain || output.id.starts_with(&further))
        .map(|output| output.id.clone())
}

/// the best card to default to: a USB DAC, else any card that is
/// not the built-in jack or HDMI (an I2S HAT). Cards without a
/// playback device are no candidates; ties go to the lower card
fn pick_output(cards: &[Card], outputs: &[Output]) -> Option<Pick> {
    cards
        .iter()
        .filter(|card| !is_builtin(card))
        .filter_map(|card| {
            Some(Pick {
                id: first_output(card, outputs)?,
                usb: is_usb(card),
            })
        })
        .min_by_key(|pick| !pick.usb)
}

fn current_pick(asound_dir: &Path) -> Option<Pick> {
    pick_output(
        &audio::list_cards(asound_dir),
        &audio::list_outputs(asound_dir),
    )
}

/// give a card that is still enumerating its time: poll until a USB
/// DAC (the best there is) is there, for at most [`SETTLE_POLLS`] steps
fn settle_output(asound_dir: &Path, runner: &mut dyn Squeezelite) {
    for poll in 0..=SETTLE_POLLS {
        if current_pick(asound_dir).is_some_and(|pick| pick.usb) {
            return;
        }
        if poll < SETTLE_POLLS {
            runner.sleep(SETTLE_STEP);
        }
    }
}

/// what the startup derives: the output (after the wait for a slow
/// DAC) and the name
fn derive(asound_dir: &Path, suffix: &str, runner: &mut dyn Squeezelite) -> Derived {
    settle_output(asound_dir, runner);
    let output = current_pick(asound_dir).map(|pick| pick.id);
    if output.is_none() {
        tracing::warn!(
            "squeezelite: no sound card besides the built-in jack and HDMI — OUTPUT stays empty, squeezelite uses its own default"
        );
    }
    Derived {
        output: output.unwrap_or_default(),
        name: default_name(suffix),
    }
}

/// a setting when it holds something, else the derived value
fn setting_or(setting: &str, derived: &str) -> String {
    let setting = setting.trim();
    if setting.is_empty() { derived } else { setting }.to_string()
}

fn compose(derived: &Derived, inputs: &Inputs) -> Env {
    Env {
        output: setting_or(&inputs.audio.output, &derived.output),
        name: setting_or(&inputs.audio.name, &derived.name),
        server: setting_or(
            &inputs.audio.server,
            &server_host(&inputs.ma_url).unwrap_or_default(),
        ),
        extra: inputs.audio.extra.trim().to_string(),
    }
}

/// characters that need no quoting in a shell word
fn is_plain(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c)
}

/// the shell word for `value`: bare when plain, else single-quoted
/// (nothing inside single quotes is special but the quote itself)
fn shell_word(value: &str) -> String {
    if value.chars().all(is_plain) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// the file as mestro writes it; a value with a line break cannot be
/// a shell word on one line
fn render(env: &Env) -> Result<String, String> {
    let mut text = HEADER.to_string();
    for (key, value) in [
        ("OUTPUT", &env.output),
        ("NAME", &env.name),
        ("SERVER", &env.server),
        ("EXTRA", &env.extra),
    ] {
        if value.contains(['\n', '\r', '\0']) {
            return Err(format!("{key} cannot hold a line break"));
        }
        text.push_str(&format!("{key}={}\n", shell_word(value)));
    }
    Ok(text)
}

/// compose the file and, only if its content changes, write it and
/// restart squeezelite (see [`Trigger`]). A first run with nothing
/// to say writes nothing: the init script starts squeezelite on its
/// own defaults without the file
fn apply(
    paths: &Paths,
    derived: &Derived,
    inputs: &Inputs,
    trigger: Trigger,
    runner: &mut dyn Squeezelite,
) -> Result<bool, String> {
    let env = compose(derived, inputs);
    let text = render(&env)?;
    let existing = std::fs::read_to_string(paths.env_file).ok();
    let unchanged = match &existing {
        Some(existing) => *existing == text,
        None => env == Env::default(),
    };
    if unchanged && trigger == Trigger::Background {
        return Ok(false);
    }
    if !unchanged {
        tracing::info!("squeezelite: {env:?}");
        crate::atomic_file::write_atomic(paths.env_file, &text)?;
    }
    // a squeezelite that is not up yet reads the file when it starts;
    // one that is starting right now may still miss it
    if trigger == Trigger::Switch || runner.running() {
        runner.restart().map_err(|e| format!("restart: {e}"))?;
        return Ok(true);
    }
    Ok(false)
}

/// wait until squeezelite runs, for at most [`START_POLLS`] steps;
/// false when it never did
fn wait_for_start(runner: &mut dyn Squeezelite) -> bool {
    for poll in 0..=START_POLLS {
        if runner.running() {
            return true;
        }
        if poll < START_POLLS {
            runner.sleep(START_STEP);
        }
    }
    false
}

/// the value after `flag` in `arguments`, empty when the flag is absent
fn flag_value<'a>(arguments: &'a [String], flag: &str) -> &'a str {
    arguments
        .iter()
        .position(|argument| argument == flag)
        .and_then(|at| arguments.get(at + 1))
        .map_or("", String::as_str)
}

/// restart squeezelite once if it runs with other `-o`/`-s`/`-n` than
/// the env's (`EXTRA` is not compared); true when it restarted
fn restart_if_arguments_differ(env: &Env, runner: &mut dyn Squeezelite) -> Result<bool, String> {
    let Some(arguments) = runner.arguments() else {
        return Ok(false);
    };
    let matches = [("-o", &env.output), ("-s", &env.server), ("-n", &env.name)]
        .iter()
        .all(|(flag, value)| flag_value(&arguments, flag) == value.as_str());
    if matches {
        return Ok(false);
    }
    tracing::info!("squeezelite started without mestro's settings, restarting");
    runner.restart().map_err(|e| format!("restart: {e}"))?;
    Ok(true)
}

/// a run in its turn: the startup run (`startup_suffix`) derives
/// first, without any lock (a slow DAC takes seconds); a background
/// run then waits for an output switch in flight, so it composes from
/// the settings as that switch left them. Every run composes from the
/// newest settings. A run before the startup's has nothing to compose
/// from — the startup run will
fn run(
    paths: &Paths,
    startup_suffix: Option<&str>,
    trigger: Trigger,
    runner: &mut dyn Squeezelite,
) -> Result<(), String> {
    if let Some(suffix) = startup_suffix {
        let derived = derive(paths.asound_dir, suffix, runner);
        *crate::locks::lock(&DERIVED) = Some(derived);
    }
    // the slot before the turn, like the switch (which holds the slot
    // when it asks for its run)
    let restarted = {
        let _slot = (trigger == Trigger::Background).then(|| SwitchSlot::wait(paths.env_file));
        let _turn = crate::locks::lock(&RUNS);
        let Some(derived) = crate::locks::lock(&DERIVED).clone() else {
            return Ok(());
        };
        let inputs = crate::locks::lock(&INPUTS).clone().unwrap_or_default();
        apply(paths, &derived, &inputs, trigger, runner)
    }?;
    if startup_suffix.is_some() && !restarted {
        confirm_startup(paths, runner)?;
    }
    Ok(())
}

/// the startup run's last step when it did not restart squeezelite
/// itself: one that the init script started before the file existed
/// runs on its own defaults, and `apply` saw it too early. Waits for
/// it without any lock, then compares with the newest settings in the
/// run's turn
fn confirm_startup(paths: &Paths, runner: &mut dyn Squeezelite) -> Result<(), String> {
    if !wait_for_start(runner) {
        return Ok(());
    }
    let _slot = SwitchSlot::wait(paths.env_file);
    let _turn = crate::locks::lock(&RUNS);
    let derived = crate::locks::lock(&DERIVED).clone().unwrap_or_default();
    let inputs = crate::locks::lock(&INPUTS).clone().unwrap_or_default();
    restart_if_arguments_differ(&compose(&derived, &inputs), runner).map(|_| ())
}

fn device_paths() -> Paths<'static> {
    Paths {
        env_file: Path::new(super::paths::SQUEEZELITE_ENV),
        asound_dir: Path::new(audio::ASOUND_DIR),
    }
}

fn set_inputs(audio: &Audio, ma_url: &str) {
    *crate::locks::lock(&INPUTS) = Some(Inputs {
        audio: audio.clone(),
        ma_url: ma_url.to_string(),
    });
}

/// a run on its own thread: the waits (a slow DAC, a switch in
/// flight) and the restart take seconds, never on the caller's thread.
/// Nothing happens where squeezelite's init script is not (a desktop)
fn spawn_run(startup_suffix: Option<String>) {
    if !audio::available(super::paths::SQUEEZELITE_RESTART) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mestro-audio-env".into())
        .spawn(move || {
            let mut runner = audio::SqueezeliteService::new(super::paths::SQUEEZELITE_RESTART);
            if let Err(e) = run(
                &device_paths(),
                startup_suffix.as_deref(),
                Trigger::Background,
                &mut runner,
            ) {
                tracing::warn!("squeezelite: env file: {e}");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("squeezelite: env thread: {e}");
    }
}

/// the startup run: the `NAME` suffix (`device_suffix`) and the
/// settings as loaded
pub fn start(suffix: &str, audio: &Audio, ma_url: &str) {
    set_inputs(audio, ma_url);
    spawn_run(Some(suffix.to_string()));
}

/// an `audio.*` or `music-assistant.*` setting changed
pub fn settings_changed(audio: &Audio, ma_url: &str) {
    set_inputs(audio, ma_url);
    spawn_run(None);
}

/// the output switch's run, on the caller's thread (which holds the
/// switch slot): the file follows the settings as they are now and
/// squeezelite restarts, a failing restart is the error
pub fn apply_for_switch(runner: &mut dyn Squeezelite) -> Result<(), String> {
    run(&device_paths(), None, Trigger::Switch, runner)
}

/// the output squeezelite plays on: the setting, else the derived one
pub fn effective_output() -> String {
    let derived = crate::locks::lock(&DERIVED).clone().unwrap_or_default();
    let inputs = crate::locks::lock(&INPUTS).clone().unwrap_or_default();
    compose(&derived, &inputs).output
}

/// the `audio.output` setting as it is now (empty = automatic)
pub fn output_setting() -> String {
    let inputs = crate::locks::lock(&INPUTS).clone().unwrap_or_default();
    inputs.audio.output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// the tests that run with the global INPUTS, DERIVED and RUNS go one at a time
    static GLOBAL_STATE_TESTS: Mutex<()> = Mutex::new(());

    const USB_DAC: &str = " 3 [S3             ]: USB-Audio - Topping S3\n\
                      Topping Topping S3 at usb-0000:01:00.0-1.2, high speed\n";
    const BUILTIN: &str = " 0 [Headphones     ]: bcm2835_headphonographic - bcm2835 Headphones\n\
                      bcm2835 Headphones\n\
 1 [vc4hdmi0       ]: vc4-hdmi - vc4-hdmi-0\n\
                      vc4-hdmi-0\n\
 2 [vc4hdmi1       ]: vc4-hdmi - vc4-hdmi-1\n\
                      vc4-hdmi-1\n";
    const HAT: &str = " 3 [sndrpihifiberry]: simple-card - snd_rpi_hifiberry_dacplus\n\
                      snd_rpi_hifiberry_dacplus\n";
    const USB_MIC: &str = " 5 [Mic            ]: USB-Audio - USB Microphone\n\
                      Generic USB Microphone at usb-0000:01:00.0-1.3, full speed\n";

    /// a fake /proc/asound: `cards` text plus one playback pcm per
    /// listed card index (`playback`) and capture-only ones (`capture`)
    fn asound(cards: &str, playback: &[u32], capture: &[u32]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("cards"), cards).unwrap();
        for card in playback {
            std::fs::create_dir_all(dir.path().join(format!("card{card}/pcm0p"))).unwrap();
        }
        for card in capture {
            std::fs::create_dir_all(dir.path().join(format!("card{card}/pcm0c"))).unwrap();
        }
        dir
    }

    fn pick(cards: &str, playback: &[u32], capture: &[u32]) -> Option<String> {
        let dir = asound(cards, playback, capture);
        current_pick(dir.path()).map(|pick| pick.id)
    }

    #[test]
    fn a_usb_dac_beats_hdmi_and_the_headphone_jack() {
        let cards = format!("{BUILTIN}{USB_DAC}");
        assert_eq!(
            pick(&cards, &[0, 1, 2, 3], &[]).as_deref(),
            Some("hw:CARD=S3")
        );
    }

    #[test]
    fn a_hat_beats_hdmi_and_the_headphone_jack() {
        let cards = format!("{BUILTIN}{HAT}");
        assert_eq!(
            pick(&cards, &[0, 1, 2, 3], &[]).as_deref(),
            Some("hw:CARD=sndrpihifiberry")
        );
    }

    #[test]
    fn a_usb_dac_beats_a_hat_whatever_the_card_order() {
        let cards = format!("{HAT}{}", USB_DAC.replace(" 3 [S3 ", " 4 [S3 "));
        assert_eq!(pick(&cards, &[3, 4], &[]).as_deref(), Some("hw:CARD=S3"));
    }

    #[test]
    fn only_the_builtin_cards_leave_nothing_to_pick() {
        assert_eq!(pick(BUILTIN, &[0, 1, 2], &[]), None);
    }

    #[test]
    fn a_capture_only_usb_card_is_no_output() {
        let cards = format!("{BUILTIN}{USB_MIC}");
        assert_eq!(pick(&cards, &[0, 1, 2], &[5]), None);
    }

    #[test]
    fn the_lower_card_wins_among_equals() {
        let second = USB_DAC.replace(" 3 [S3 ", " 4 [Other ");
        let cards = format!("{USB_DAC}{second}");
        assert_eq!(pick(&cards, &[3, 4], &[]).as_deref(), Some("hw:CARD=S3"));
    }

    /// squeezelite's stand-in: counts restarts, can "enumerate" a
    /// card while a startup run waits
    struct Fake {
        restarts: u32,
        restart_fails: bool,
        alive: bool,
        /// the arguments the running squeezelite was started with
        args: Vec<String>,
        /// squeezelite comes up at this start poll (never when none)
        starts_at_poll: Option<u32>,
        settle_polls: u32,
        start_polls: u32,
        on_settle_poll: Box<dyn FnMut(u32)>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                restarts: 0,
                restart_fails: false,
                alive: true,
                args: Vec::new(),
                starts_at_poll: None,
                settle_polls: 0,
                start_polls: 0,
                on_settle_poll: Box::new(|_| {}),
            }
        }
    }

    impl Squeezelite for Fake {
        fn restart(&mut self) -> Result<(), String> {
            self.restarts += 1;
            if self.restart_fails {
                Err("init script exited with 1".into())
            } else {
                Ok(())
            }
        }
        fn running(&mut self) -> bool {
            self.alive
        }
        fn arguments(&mut self) -> Option<Vec<String>> {
            self.alive.then(|| self.args.clone())
        }
        fn sleep(&mut self, duration: Duration) {
            if duration == START_STEP {
                self.start_polls += 1;
                if self.starts_at_poll == Some(self.start_polls) {
                    self.alive = true;
                }
            } else {
                assert_eq!(duration, SETTLE_STEP);
                self.settle_polls += 1;
                (self.on_settle_poll)(self.settle_polls);
            }
        }
    }

    /// the file mestro writes for these values
    fn file(output: &str, name: &str, server: &str, extra: &str) -> String {
        format!("{HEADER}OUTPUT={output}\nNAME={name}\nSERVER={server}\nEXTRA={extra}\n")
    }

    fn settings(output: &str, name: &str, server: &str, extra: &str, ma_url: &str) -> Inputs {
        Inputs {
            audio: Audio {
                output: output.into(),
                name: name.into(),
                server: server.into(),
                extra: extra.into(),
            },
            ma_url: ma_url.into(),
        }
    }

    struct Fixture {
        _asound: tempfile::TempDir,
        _dir: tempfile::TempDir,
        env_file: PathBuf,
        asound_dir: PathBuf,
    }

    impl Fixture {
        fn new(cards: &str, playback: &[u32]) -> Self {
            let asound = asound(cards, playback, &[]);
            let dir = tempfile::tempdir().unwrap();
            Self {
                env_file: dir.path().join("state/mestro/squeezelite.env"),
                asound_dir: asound.path().to_path_buf(),
                _asound: asound,
                _dir: dir,
            }
        }
        fn paths(&self) -> Paths<'_> {
            Paths {
                env_file: &self.env_file,
                asound_dir: &self.asound_dir,
            }
        }
        /// what the startup derives on this machine
        fn derived(&self, suffix: &str) -> Derived {
            derive(&self.asound_dir, suffix, &mut Fake::new())
        }
        fn apply(&self, derived: &Derived, inputs: &Inputs, runner: &mut Fake) {
            apply(&self.paths(), derived, inputs, Trigger::Background, runner).unwrap();
        }
        fn text(&self) -> String {
            std::fs::read_to_string(&self.env_file).unwrap()
        }
    }

    fn usb_fixture() -> Fixture {
        Fixture::new(&format!("{BUILTIN}{USB_DAC}"), &[0, 1, 2, 3])
    }

    #[test]
    fn without_settings_the_file_holds_the_derived_values_and_squeezelite_restarts_once() {
        let fx = usb_fixture();
        let mut fake = Fake::new();
        let derived = derive(&fx.asound_dir, "6BEC", &mut fake);
        assert_eq!(fake.settle_polls, 0, "a USB DAC needs no wait");
        fx.apply(
            &derived,
            &settings("", "", "", "", "http://192.168.3.13:8095"),
            &mut fake,
        );
        assert_eq!(
            fx.text(),
            file("hw:CARD=S3", "mestro-6BEC", "192.168.3.13", "")
        );
        assert_eq!(fake.restarts, 1, "all values, one restart");
    }

    #[test]
    fn a_non_empty_setting_beats_the_derived_value() {
        let fx = usb_fixture();
        let derived = fx.derived("6BEC");
        let mut fake = Fake::new();
        fx.apply(
            &derived,
            &settings(
                "hw:CARD=Mine",
                "HiFi",
                "192.168.3.13",
                "-r -Z 100",
                "http://10.0.0.5:8095",
            ),
            &mut fake,
        );
        assert_eq!(
            fx.text(),
            file("hw:CARD=Mine", "HiFi", "192.168.3.13", "'-r -Z 100'")
        );
    }

    #[test]
    fn a_setting_of_one_key_leaves_the_other_derived_values_alone() {
        let fx = usb_fixture();
        let derived = fx.derived("6BEC");
        let mut fake = Fake::new();
        fx.apply(
            &derived,
            &settings("", "HiFi", "", "", "http://10.0.0.5:8095"),
            &mut fake,
        );
        assert_eq!(fx.text(), file("hw:CARD=S3", "HiFi", "10.0.0.5", ""));
    }

    #[test]
    fn blank_settings_count_as_empty() {
        let fx = usb_fixture();
        let derived = fx.derived("6BEC");
        let mut fake = Fake::new();
        fx.apply(&derived, &settings("  ", " ", "\t", " ", ""), &mut fake);
        assert_eq!(fx.text(), file("hw:CARD=S3", "mestro-6BEC", "", ""));
    }

    #[test]
    fn a_setting_change_rewrites_the_file_and_restarts_once() {
        let fx = usb_fixture();
        let derived = fx.derived("6BEC");
        let mut fake = Fake::new();
        let url = "http://192.168.3.13:8095";
        fx.apply(&derived, &settings("", "", "", "", url), &mut fake);
        fx.apply(
            &derived,
            &settings("", "", "", "", "http://10.0.0.5:8095"),
            &mut fake,
        );
        assert_eq!(fx.text(), file("hw:CARD=S3", "mestro-6BEC", "10.0.0.5", ""));
        assert_eq!(fake.restarts, 2);
        // the same again: nothing
        fx.apply(
            &derived,
            &settings("", "", "", "", "http://10.0.0.5:8095"),
            &mut fake,
        );
        assert_eq!(fake.restarts, 2);
        // the url is gone (a factory reset): the host goes with it
        fx.apply(&derived, &settings("", "", "", "", ""), &mut fake);
        assert_eq!(fx.text(), file("hw:CARD=S3", "mestro-6BEC", "", ""));
        assert_eq!(fake.restarts, 3);
        // a server setting beats the url's host
        fx.apply(&derived, &settings("", "", "nas.lan", "", url), &mut fake);
        assert_eq!(fx.text(), file("hw:CARD=S3", "mestro-6BEC", "nas.lan", ""));
    }

    #[test]
    fn unchanged_values_are_not_rewritten_and_do_not_restart() {
        let fx = usb_fixture();
        let derived = fx.derived("6BEC");
        let mut fake = Fake::new();
        let inputs = settings("", "", "", "", "http://192.168.3.13:8095");
        fx.apply(&derived, &inputs, &mut fake);
        let before = std::fs::metadata(&fx.env_file).unwrap().modified().unwrap();
        // the normal next boot
        fx.apply(&derived, &inputs, &mut fake);
        assert_eq!(fake.restarts, 1);
        assert_eq!(
            std::fs::metadata(&fx.env_file).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn a_file_of_stale_content_is_replaced_whole() {
        let fx = usb_fixture();
        std::fs::create_dir_all(fx.env_file.parent().unwrap()).unwrap();
        std::fs::write(&fx.env_file, "OUTPUT=hw:CARD=Old\nSTALE=junk\n").unwrap();
        fx.apply(
            &fx.derived(""),
            &settings("", "", "", "", ""),
            &mut Fake::new(),
        );
        assert_eq!(fx.text(), file("hw:CARD=S3", "", "", ""));
    }

    #[test]
    fn only_builtin_cards_and_nothing_else_to_say_write_no_file_and_no_restart() {
        let fx = Fixture::new(BUILTIN, &[0, 1, 2]);
        let mut fake = Fake::new();
        let derived = derive(&fx.asound_dir, "", &mut fake);
        assert_eq!(fake.settle_polls, SETTLE_POLLS, "a slow DAC is waited for");
        fx.apply(&derived, &settings("", "", "", "", ""), &mut fake);
        assert!(!fx.env_file.exists());
        assert_eq!(fake.restarts, 0);
    }

    #[test]
    fn only_builtin_cards_leave_output_empty_but_the_rest_still_applies() {
        let fx = Fixture::new(BUILTIN, &[0, 1, 2]);
        let mut fake = Fake::new();
        fx.apply(
            &fx.derived("6BEC"),
            &settings("", "", "", "", "http://192.168.3.13:8095"),
            &mut fake,
        );
        assert_eq!(fx.text(), file("", "mestro-6BEC", "192.168.3.13", ""));
        assert_eq!(fake.restarts, 1);
    }

    #[test]
    fn a_hat_waits_for_a_late_usb_dac_then_takes_the_hat() {
        let fx = Fixture::new(&format!("{BUILTIN}{HAT}"), &[0, 1, 2, 3]);
        let mut fake = Fake::new();
        let derived = derive(&fx.asound_dir, "", &mut fake);
        assert_eq!(derived.output, "hw:CARD=sndrpihifiberry");
        assert_eq!(fake.settle_polls, SETTLE_POLLS, "the whole window ran");
    }

    #[test]
    fn a_usb_dac_that_shows_up_while_waiting_wins_over_the_hat() {
        let fx = Fixture::new(&format!("{BUILTIN}{HAT}"), &[0, 1, 2, 3]);
        let mut fake = Fake::new();
        let dir = fx.asound_dir.clone();
        fake.on_settle_poll = Box::new(move |poll| {
            if poll == 3 {
                let cards = std::fs::read_to_string(dir.join("cards")).unwrap();
                let usb = USB_DAC.replace(" 3 [S3 ", " 4 [S3 ");
                std::fs::write(dir.join("cards"), format!("{cards}{usb}")).unwrap();
                std::fs::create_dir_all(dir.join("card4/pcm0p")).unwrap();
            }
        });
        let derived = derive(&fx.asound_dir, "", &mut fake);
        assert_eq!(derived.output, "hw:CARD=S3");
        assert_eq!(fake.settle_polls, 3, "it stops as soon as the DAC is there");
    }

    #[test]
    fn a_failed_restart_keeps_the_file_and_is_not_retried() {
        let fx = usb_fixture();
        let mut fake = Fake::new();
        fake.restart_fails = true;
        let inputs = settings("", "", "", "", "");
        let derived = fx.derived("");
        let result = apply(
            &fx.paths(),
            &derived,
            &inputs,
            Trigger::Background,
            &mut fake,
        );
        assert!(result.unwrap_err().contains("restart"));
        assert_eq!(fx.text(), file("hw:CARD=S3", "", "", ""));
        assert_eq!(fake.restarts, 1);
    }

    #[test]
    fn a_squeezelite_that_is_not_running_is_not_restarted_but_the_file_is_written() {
        let fx = usb_fixture();
        let mut fake = Fake::new();
        fake.alive = false;
        fx.apply(&fx.derived(""), &settings("", "", "", "", ""), &mut fake);
        assert_eq!(fx.text(), file("hw:CARD=S3", "", "", ""));
        assert_eq!(fake.restarts, 0);
    }

    #[test]
    fn a_switch_restarts_squeezelite_even_when_it_is_down_or_nothing_changed() {
        let fx = usb_fixture();
        let derived = fx.derived("");
        let inputs = settings("hw:CARD=S3", "", "", "", "");
        let mut fake = Fake::new();
        fake.alive = false;
        apply(&fx.paths(), &derived, &inputs, Trigger::Switch, &mut fake).unwrap();
        assert_eq!(fake.restarts, 1);
        apply(&fx.paths(), &derived, &inputs, Trigger::Switch, &mut fake).unwrap();
        assert_eq!(fake.restarts, 2, "unchanged file, still restarted");
        fake.restart_fails = true;
        let err = apply(&fx.paths(), &derived, &inputs, Trigger::Switch, &mut fake).unwrap_err();
        assert!(err.contains("restart"), "{err}");
    }

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    fn s3_env() -> Env {
        Env {
            output: "hw:CARD=S3".into(),
            name: "HiFi".into(),
            server: "192.168.3.13".into(),
            extra: String::new(),
        }
    }

    #[test]
    fn a_squeezelite_started_without_the_settings_is_restarted_once() {
        let mut fake = Fake::new();
        fake.args = argv(&["-v"]);
        assert!(restart_if_arguments_differ(&s3_env(), &mut fake).unwrap());
        assert_eq!(fake.restarts, 1);
    }

    #[test]
    fn a_squeezelite_started_with_the_settings_is_left_alone() {
        let mut fake = Fake::new();
        fake.args = argv(&["-v", "-o", "hw:CARD=S3", "-s", "192.168.3.13", "-n", "HiFi"]);
        assert!(!restart_if_arguments_differ(&s3_env(), &mut fake).unwrap());
        assert_eq!(fake.restarts, 0);
    }

    #[test]
    fn each_of_output_server_and_name_is_compared() {
        for wrong in [
            ["-o", "hw:CARD=Other", "-s", "192.168.3.13", "-n", "HiFi"],
            ["-o", "hw:CARD=S3", "-s", "10.0.0.5", "-n", "HiFi"],
            ["-o", "hw:CARD=S3", "-s", "192.168.3.13", "-n", "Kitchen"],
        ] {
            let mut fake = Fake::new();
            fake.args = argv(&wrong);
            assert!(restart_if_arguments_differ(&s3_env(), &mut fake).unwrap());
        }
        // a flag the file leaves empty must be absent
        let mut fake = Fake::new();
        fake.args = argv(&["-o", "hw:CARD=S3", "-n", "HiFi"]);
        let no_server = Env {
            server: String::new(),
            ..s3_env()
        };
        assert!(!restart_if_arguments_differ(&no_server, &mut fake).unwrap());
        fake.args = argv(&["-o", "hw:CARD=S3", "-s", "x", "-n", "HiFi"]);
        assert!(restart_if_arguments_differ(&no_server, &mut fake).unwrap());
    }

    #[test]
    fn extra_flags_are_not_compared() {
        let mut fake = Fake::new();
        fake.args = argv(&[
            "-v",
            "-o",
            "hw:CARD=S3",
            "-s",
            "192.168.3.13",
            "-n",
            "HiFi",
            "-r",
            "44100",
        ]);
        let env = Env {
            extra: "-a 80:4".into(),
            ..s3_env()
        };
        assert!(!restart_if_arguments_differ(&env, &mut fake).unwrap());
        // and with nothing set, the bare `-v` start is the right one
        fake.args = argv(&["-v", "-a", "80"]);
        assert!(!restart_if_arguments_differ(&Env::default(), &mut fake).unwrap());
        assert_eq!(fake.restarts, 0);
    }

    #[test]
    fn no_running_squeezelite_is_not_restarted_for_its_arguments() {
        let mut fake = Fake::new();
        fake.alive = false;
        assert!(!restart_if_arguments_differ(&s3_env(), &mut fake).unwrap());
        assert_eq!(fake.restarts, 0);
    }

    #[test]
    fn the_start_wait_ends_as_soon_as_squeezelite_runs() {
        let mut fake = Fake::new();
        fake.alive = false;
        fake.starts_at_poll = Some(7);
        assert!(wait_for_start(&mut fake));
        assert_eq!(fake.start_polls, 7);
        let mut running = Fake::new();
        assert!(wait_for_start(&mut running));
        assert_eq!(running.start_polls, 0, "nothing to wait for");
    }

    #[test]
    fn the_start_wait_is_bounded_when_squeezelite_never_runs() {
        let mut fake = Fake::new();
        fake.alive = false;
        assert!(!wait_for_start(&mut fake));
        assert_eq!(fake.start_polls, START_POLLS);
        assert!(START_STEP * START_POLLS >= Duration::from_secs(40));
    }

    #[test]
    fn the_name_is_mestro_and_the_suffix_none_without_one() {
        assert_eq!(default_name("6BEC"), "mestro-6BEC");
        assert_eq!(default_name(""), "");
    }

    #[test]
    fn the_server_is_the_host_of_the_ma_url() {
        for (url, host) in [
            ("http://192.168.3.13:8095", Some("192.168.3.13")),
            ("https://ma.example.org", Some("ma.example.org")),
            ("ws://ma.local:8095/ws", Some("ma.local")),
            ("wss://ma.local/ws/", Some("ma.local")),
            ("  http://ma:8095/  ", Some("ma")),
            ("192.168.3.13:8095", Some("192.168.3.13")),
            ("http://user:pw@ma.lan:8095/x?y#z", Some("ma.lan")),
            ("", None),
            ("   ", None),
            ("http://", None),
            ("http://[fe80::1]:8095", None),
            ("http://ma host", None),
            ("http://:8095", None),
        ] {
            assert_eq!(server_host(url).as_deref(), host, "{url:?}");
        }
    }

    /// what a shell sees after sourcing `text`: OUTPUT, NAME, SERVER, EXTRA
    fn sourced(text: &str) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("squeezelite.env");
        std::fs::write(&file, text).unwrap();
        let out = std::process::Command::new("sh")
            .args([
                "-c",
                ". \"$0\"; printf '%s\\n' \"$OUTPUT\" \"$NAME\" \"$SERVER\" \"$EXTRA\"",
            ])
            .arg(&file)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    }

    #[test]
    fn the_shell_reads_back_what_mestro_wrote() {
        for env in [
            Env::default(),
            Env {
                output: "hw:CARD=S3,DEV=0".into(),
                name: "mestro-6BEC".into(),
                server: "192.168.3.13".into(),
                extra: "-r -Z 100".into(),
            },
            Env {
                output: "plughw:CARD=a b".into(),
                name: "Wohnzimmer's \"HiFi\" $HOME `id` \\ &;|<>*".into(),
                server: "ma.local".into(),
                extra: "-a 80:4 -D 'x y'".into(),
            },
        ] {
            let seen = sourced(&render(&env).unwrap());
            assert_eq!(seen, [env.output, env.name, env.server, env.extra]);
        }
    }

    #[test]
    fn a_value_with_a_line_break_is_refused() {
        for bad in ["a\nb", "a\rb", "a\0b"] {
            let env = Env {
                name: bad.into(),
                ..Env::default()
            };
            assert!(render(&env).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_newest_settings_win_on_sequential_runs() {
        let _serial = crate::locks::lock(&GLOBAL_STATE_TESTS);
        let fx = usb_fixture();
        let mut fake = Fake::new();
        set_inputs(&Audio::default(), "");
        run(&fx.paths(), Some("6BEC"), Trigger::Background, &mut fake).unwrap();
        assert_eq!(effective_output(), "hw:CARD=S3");
        assert_eq!(output_setting(), "");
        set_inputs(&Audio::default(), "http://10.0.0.5:8095");
        set_inputs(
            &Audio {
                output: "hw:CARD=Mine".into(),
                ..Audio::default()
            },
            "http://10.0.0.6:8095",
        );
        run(&fx.paths(), None, Trigger::Background, &mut fake).unwrap();
        run(&fx.paths(), None, Trigger::Background, &mut fake).unwrap();
        assert_eq!(
            fx.text(),
            file("hw:CARD=Mine", "mestro-6BEC", "10.0.0.6", "")
        );
        assert_eq!(effective_output(), "hw:CARD=Mine");
        assert_eq!(output_setting(), "hw:CARD=Mine");
        assert_eq!(
            fake.restarts, 2,
            "the startup's, one for the settings; the second run did nothing"
        );
        // a switch run follows the settings too
        run(&fx.paths(), None, Trigger::Switch, &mut fake).unwrap();
        assert_eq!(fake.restarts, 3);
    }

    /// a startup run on a fresh install: the file is written while
    /// squeezelite is not up yet (it comes up at start poll `up_at`
    /// with `args`); the restarts it takes
    fn first_boot_restarts(up_at: Option<u32>, args: &[&str]) -> u32 {
        let fx = usb_fixture();
        let mut fake = Fake::new();
        fake.alive = false;
        fake.starts_at_poll = up_at;
        fake.args = argv(args);
        set_inputs(
            &Audio {
                name: "HiFi".into(),
                extra: "-a 80".into(),
                ..Audio::default()
            },
            "http://192.168.3.13:8095",
        );
        run(&fx.paths(), Some("6BEC"), Trigger::Background, &mut fake).unwrap();
        assert_eq!(
            fx.text(),
            file("hw:CARD=S3", "HiFi", "192.168.3.13", "'-a 80'")
        );
        assert!(fake.start_polls <= START_POLLS);
        fake.restarts
    }

    #[test]
    fn a_squeezelite_that_started_before_the_file_is_restarted_once() {
        let _serial = crate::locks::lock(&GLOBAL_STATE_TESTS);
        assert_eq!(first_boot_restarts(Some(3), &["-v"]), 1);
        let matching = ["-v", "-o", "hw:CARD=S3", "-s", "192.168.3.13", "-n", "HiFi"];
        assert_eq!(first_boot_restarts(Some(3), &matching), 0);
        let mut other_extra = matching.to_vec();
        other_extra.extend(["-r", "44100"]);
        assert_eq!(first_boot_restarts(Some(3), &other_extra), 0);
        assert_eq!(
            first_boot_restarts(None, &["-v"]),
            0,
            "never up: no restart"
        );
    }
}
