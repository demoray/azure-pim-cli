use anyhow::{ensure, Context, Result};
use home::home_dir;
use reqwest::{blocking::Client, header::USER_AGENT};
use semver::Version;
use serde_json::Value;
use std::{
    cmp::Ordering,
    fs::{create_dir_all, metadata, read, write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::{debug, info, trace};

const CACHE_EXPIRE: Duration = Duration::from_secs(3600);
const RELEASES_API_URL: &str = "https://api.github.com/repos/demoray/azure-pim-cli/releases/latest";
const RELEASES_URL: &str = "https://github.com/demoray/azure-pim-cli/releases";

fn cache_path() -> Option<PathBuf> {
    home_dir().map(|x| x.join(".cache").join("az-pim-cli"))
}

fn read_cached_latest(path: &Path) -> Result<Version> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?;
    let duration = now.saturating_sub(metadata(path)?.modified()?.duration_since(UNIX_EPOCH)?);
    ensure!(
        duration < CACHE_EXPIRE,
        "cache file is too old ({duration:?})"
    );

    let content = read(path).context("unable to read cache file")?;
    let as_str = String::from_utf8(content).context("unable to parse as utf8 string")?;
    Version::parse(&as_str).context("unable to parse cache file")
}

/// Check if the cache file is older than 1 hour
///
/// # Returns
/// * Does not check if `$HOME` cannot be determined
/// * Does not check if `$HOME/.cache/az-pim-cli` cannot be created
pub fn check_latest_version() -> Result<()> {
    let current =
        Version::parse(env!("CARGO_PKG_VERSION")).context("unable to parse current version")?;

    let cache_path = cache_path().context("unable to determine cache path")?;
    create_dir_all(&cache_path).context("unable to create cache path")?;

    let cache_file_path = cache_path.join("latest.version");

    match read_cached_latest(&cache_file_path) {
        Ok(cached_latest) => match cached_latest.cmp(&current) {
            Ordering::Less => {
                debug!("cached latest is older than current: {cached_latest} < {current}");
            }
            Ordering::Greater => {
                info!("a new version of az-pim ({cached_latest}) is available at {RELEASES_URL} (using {current})");
                return Ok(());
            }
            Ordering::Equal => {
                debug!("from cache az-pim is up-to-date: {current} >= {cached_latest}");
                return Ok(());
            }
        },
        Err(e) => {
            debug!("unable to check cached version of az-pim: {e}");
        }
    }

    let text = Client::new()
        .get(RELEASES_API_URL)
        .header(USER_AGENT, "az-pim-cli")
        .send()
        .context("unable to send request to GitHub")?
        .text()
        .context("unable to receive response from GitHub")?;
    trace!("response: {text:?}");
    let response: Value = serde_json::from_str(&text).context("unable to deserialize response")?;

    let tag = response
        .get("tag_name")
        .context("missing field tag_name")?
        .as_str()
        .context("tag_name is not a string")?;

    let latest = Version::parse(tag).context("unable to parse tag_name")?;
    if latest > current {
        info!(
            "a new version of az-pim ({latest}) is available at {RELEASES_URL} (using {current})"
        );
    } else {
        debug!("from live az-pim is up-to-date: {current} >= {latest}");
    }

    write(cache_file_path, tag).context("unable to write cache file")?;

    Ok(())
}
