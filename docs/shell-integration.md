# Shell integration (OSC 133)

MahzenTerm understands the prompt marks that modern shells can print. With them you get:

- **Ctrl+Shift+Up / Ctrl+Shift+Down** (or the palette: "Jump to Previous/Next Prompt") to move between
  commands in the scrollback. Past the last prompt you land back at the bottom.
- the `exit` status-bar widget: a red `EXIT 1` chip while the last command failed (on by default).

Nothing is installed for you: add one of these to your shell's startup file. The marks are invisible in
terminals that do not know them.

## zsh (`~/.zshrc`)

```zsh
autoload -Uz add-zsh-hook
_mt_precmd()  { printf '\e]133;D;%s\a\e]133;A\a' "$?"; }
_mt_preexec() { printf '\e]133;C\a'; }
add-zsh-hook precmd  _mt_precmd
add-zsh-hook preexec _mt_preexec
```

## bash (`~/.bashrc`)

```bash
PROMPT_COMMAND='printf "\e]133;D;%s\a\e]133;A\a" "$?"'"${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
```

## fish (`~/.config/fish/config.fish`)

```fish
function _mt_prompt --on-event fish_prompt
    printf '\e]133;D;%s\a\e]133;A\a' $status
end
function _mt_preexec --on-event fish_preexec
    printf '\e]133;C\a'
end
```

## Limits

Prompt positions are forgotten when a pane is resized (text reflows) or its scrollback fills up
(10 000 lines), because the stored rows would no longer match. Full-screen programs (alternate screen)
are ignored. Jumping is per pane.

## Working directory (OSC 7)

Shells that print `ESC ] 7 ; file://host/path ST` after `cd` (zsh with `chpwd`, fish, bash with `PROMPT_COMMAND`) tell
MahzenTerm where they are. New tabs and splits normally ask the OS for the shell's directory; the OSC 7 report is the
fallback (for example when the OS refuses). A report whose host is not this machine (an ssh session) names a remote
path and is never used for a local pane. Example for bash:

```sh
PROMPT_COMMAND='printf "\033]7;file://%s%s\007" "$HOSTNAME" "$PWD"'
```
