// The image's build-time variables are read with option_env! in
// the module that owns each of them; a changed variable must
// recompile. Every module's guard test
// (build_rs_reruns_on_every_variable_read_here) pins this list to
// the source. Listed in CONTRIBUTING.md "Building for another
// image".
const IMAGE_VARS: &[&str] = &[
    // wifi::paths
    "MESTRO_WPA_CTRL_DIR",
    "MESTRO_WPA_CONF",
    "MESTRO_WPA_TEMPLATE",
    // audio::squeezelite
    "MESTRO_SQUEEZELITE_ENV",
    "MESTRO_SQUEEZELITE_RESTART",
    // timezone
    "MESTRO_ENV_FILE",
    "MESTRO_ZONEINFO_DIR",
    "MESTRO_LOCALTIME",
    // logs
    "MESTRO_LOG_DIR",
    "MESTRO_STATUS_COMMAND",
];

fn main() {
    for var in IMAGE_VARS {
        println!("cargo:rerun-if-env-changed={var}");
    }
}
