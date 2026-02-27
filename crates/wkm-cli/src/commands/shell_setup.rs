use clap::Args;

#[derive(Args)]
pub struct ShellSetupArgs {
    /// Shell type (bash, zsh, fish). Auto-detected from $SHELL if omitted.
    #[arg(long)]
    pub shell: Option<String>,
}

pub fn run(args: &ShellSetupArgs) -> anyhow::Result<()> {
    let shell = match &args.shell {
        Some(s) => s.clone(),
        None => detect_shell()?,
    };

    let output = match shell.as_str() {
        "bash" | "zsh" => BASH_ZSH_WRAPPER,
        "fish" => FISH_WRAPPER,
        other => anyhow::bail!("unsupported shell: {other}. Supported: bash, zsh, fish"),
    };

    print!("{output}");
    Ok(())
}

fn detect_shell() -> anyhow::Result<String> {
    let shell_env = std::env::var("SHELL")
        .map_err(|_| anyhow::anyhow!("$SHELL not set; pass --shell explicitly"))?;
    let name = std::path::Path::new(&shell_env)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    Ok(name)
}

const BASH_ZSH_WRAPPER: &str = r#"export WKM_SHELL_SETUP=1
wkm() {
  if [ "$1" = "wp" ] && [ $# -ge 2 ]; then
    local dir
    dir="$(command wkm worktree-path "${@:2}")" && cd "$dir"
  else
    command wkm "$@"
  fi
}
"#;

const FISH_WRAPPER: &str = r#"set -gx WKM_SHELL_SETUP 1
function wkm
  if test (count $argv) -ge 2; and test "$argv[1]" = "wp"
    set -l dir (command wkm worktree-path $argv[2..])
    and cd $dir
  else
    command wkm $argv
  end
end
"#;
