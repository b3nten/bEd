//! Shell integration: the user's own shell (no explicit `shell` in the
//! options) marks its prompts and commands with OSC 133 — `A` where a prompt
//! starts, `C` where a command's output starts, `D;status` when it finished —
//! so the terminal knows where each command is and how it went. zsh also
//! reports its directory (OSC 7) at each prompt, for relative paths.
//!
//! - zsh: `ZDOTDIR` points at bed's `.zshenv`, which puts the user's
//!   `ZDOTDIR` back, loads their `.zshenv`, and adds a precmd and a preexec
//!   hook. zsh then reads the rest of the user's startup files as usual.
//! - bash: `PS0` (bash 4.4+) and `PROMPT_COMMAND` from the environment. A
//!   `.bashrc` that replaces `PROMPT_COMMAND` outright loses the marks.
//! - fish 4 marks prompts itself.
use crate::TerminalOptions;
use std::{
    io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

const ZSHENV: &str = r#"# bed's shell integration (bedtime-terminal): marks prompts and commands
# (OSC 133) and the directory (OSC 7) for bed's terminal, after putting your
# ZDOTDIR back and loading your .zshenv. The rest of your startup files load
# as usual.
if [[ -n "${BEDTIME_ZDOTDIR-}" ]]; then
  ZDOTDIR="$BEDTIME_ZDOTDIR"
else
  builtin unset ZDOTDIR
fi
builtin unset BEDTIME_ZDOTDIR
[[ -f "${ZDOTDIR:-$HOME}/.zshenv" ]] && builtin source "${ZDOTDIR:-$HOME}/.zshenv"
if [[ -o interactive ]]; then
  _bedtime_precmd() {
    local ret=$?
    if [[ -n "${_bedtime_running-}" ]]; then
      builtin printf '\e]133;D;%s\a' "$ret"
    fi
    _bedtime_running=
    builtin printf '\e]7;file://%s%s\a' "${HOST}" "${PWD// /%20}"
    builtin printf '\e]133;A\a'
  }
  _bedtime_preexec() {
    _bedtime_running=1
    builtin printf '\e]133;C\a'
  }
  builtin autoload -Uz add-zsh-hook
  add-zsh-hook precmd _bedtime_precmd
  add-zsh-hook preexec _bedtime_preexec
fi
"#;

/// Mark the user's shell's prompts and commands, if it's one we know.
pub(crate) fn apply(options: &mut TerminalOptions) {
    if options.shell.is_some() {
        return;
    }
    let program = std::env::var("SHELL").unwrap_or_default();
    let name = Path::new(&program)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.as_str() {
        "zsh" => {
            let Ok(directory) = zsh_directory() else {
                return;
            };
            let original = options
                .env
                .get("ZDOTDIR")
                .cloned()
                .or_else(|| std::env::var("ZDOTDIR").ok());
            if let Some(original) = original {
                options.env.insert("BEDTIME_ZDOTDIR".into(), original);
            }
            options
                .env
                .insert("ZDOTDIR".into(), directory.to_string_lossy().into_owned());
        }
        "bash" => {
            options.env.insert("PS0".into(), "\x1b]133;C\x07".into());
            options.env.insert(
                "PROMPT_COMMAND".into(),
                r#"printf '\e]133;D;%s\a\e]133;A\a' "$?""#.into(),
            );
        }
        _ => {}
    }
}

/// bed's `.zshenv`, written once per process to a private directory.
fn zsh_directory() -> io::Result<&'static PathBuf> {
    static DIRECTORY: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    DIRECTORY
        .get_or_init(|| {
            let directory = std::env::temp_dir()
                .join(format!("bedtime-shell-{}", std::process::id()))
                .join("zsh");
            write_zshenv(&directory)
                .map(|()| directory)
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|error| io::Error::other(error.clone()))
}

fn write_zshenv(directory: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    std::fs::write(directory.join(".zshenv"), ZSHENV)
}
