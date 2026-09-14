use std::path::{Path, PathBuf};

use clap::Parser;

use ncoxide::app::App;
use ncoxide::config::Config;

/// ncoxide — Modal dual-pane file commander
#[derive(Parser)]
#[command(name = "ncoxide", version, about)]
struct Cli {
    /// Left pane starting directory
    #[arg(short, long)]
    left: Option<PathBuf>,

    /// Right pane starting directory
    #[arg(short, long)]
    right: Option<PathBuf>,

    /// Log file path (defaults to $XDG_STATE_HOME/ncoxide/ncoxide.log)
    #[arg(long)]
    log: Option<PathBuf>,

    /// Report the terminal's image protocol and cell size, then exit
    #[arg(long)]
    probe_terminal: bool,
}

/// Default log location: the XDG state dir, not a predictable name in
/// world-writable /tmp (multi-user clobber / pre-creation hazard).
fn default_log_path() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("ncoxide")
        .join("ncoxide.log")
}

fn main() {
    let cli = Cli::parse();

    // Set up logging
    let log_path = cli.log.unwrap_or_else(default_log_path);
    if let Err(e) = setup_logging(&log_path) {
        eprintln!("Failed to set up logging: {e}");
    }

    let config = Config::load();

    if cli.probe_terminal {
        print!("{}", ncoxide::graphics::report(&config.preview));
        return;
    }

    // Starting directory precedence: CLI flag > config > current dir.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let left_path = cli
        .left
        .or_else(|| config.general.left_dir.clone())
        .unwrap_or_else(|| cwd.clone());
    let right_path = cli
        .right
        .or_else(|| config.general.right_dir.clone())
        .unwrap_or(cwd);

    let mut app = App::new_with_config(left_path, right_path, config);
    if let Err(e) = app.run() {
        eprintln!("ncoxide error: {e}");
        std::process::exit(1);
    }
}

fn setup_logging(log_path: &Path) -> Result<(), fern::InitError> {
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{} {} {}] {}",
                jiff::Zoned::now().strftime("%Y-%m-%d %H:%M:%S"),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .chain(fern::log_file(log_path)?)
        .apply()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_log_path_is_namespaced() {
        let path = default_log_path();
        assert!(path.ends_with("ncoxide/ncoxide.log"), "{path:?}");
        assert_ne!(path, PathBuf::from("/tmp/ncoxide.log"));
    }
}
