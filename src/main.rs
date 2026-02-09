use std::path::PathBuf;

use clap::Parser;

use ncoxide::app::App;

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

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let left_path = cli.left.unwrap_or_else(|| cwd.clone());
    let right_path = cli.right.unwrap_or(cwd);

    let mut app = App::new(left_path, right_path);
    if let Err(e) = app.run() {
        eprintln!("ncoxide error: {e}");
        std::process::exit(1);
    }
}

fn setup_logging(log_path: &PathBuf) -> Result<(), fern::InitError> {
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
