use anyhow::Result;
use clap::{ArgAction, Args};
use std::io::stderr;
use tracing::Level;

#[derive(Args)]
#[command(about = None)]
pub struct Verbosity {
    /// Increase logging verbosity.  Provide repeatedly to increase the verbosity.
    #[clap(long, action = ArgAction::Count, global = true)]
    verbose: u8,

    /// Only show errors
    #[clap(long, global = true, conflicts_with = "verbose")]
    quiet: bool,
}

impl Verbosity {
    fn get_level(&self) -> Level {
        if self.quiet {
            Level::ERROR
        } else {
            match self.verbose {
                0 => Level::INFO,
                1 => Level::DEBUG,
                _ => Level::TRACE,
            }
        }
    }
}

pub fn setup_logging(verbose: &Verbosity) -> Result<()> {
    let filter = if let Ok(x) = tracing_subscriber::EnvFilter::try_from_default_env() {
        x
    } else {
        tracing_subscriber::EnvFilter::builder().parse(verbose.get_level().as_str())?
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(stderr)
        .try_init()
        .ok();

    Ok(())
}
