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

    /// Log file path
    #[arg(long, default_value = "/tmp/ncoxide.log")]
    log: PathBuf,
}

fn main() {
    let cli = Cli::parse();

    // Set up logging
    if let Err(e) = setup_logging(&cli.log) {
        eprintln!("Failed to set up logging: {e}");
    }

    let config = Config::load();

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
        .level(log::LevelFilter::Debug)
        .chain(fern::log_file(log_path)?)
        .apply()?;
    Ok(())
}
