use anyhow::Result;
use std::process::Command;
use uuid::Uuid;

fn az_cmd(args: &[&str]) -> Result<String> {
    let output = Command::new("az").args(args).output()?;
    let output = String::from_utf8(output.stdout)?;
    Ok(output.trim().to_string())
}

pub fn get_token() -> Result<String> {
    az_cmd(&[
        "account",
        "get-access-token",
        "--scope=https://management.core.windows.net/.default",
        "--query",
        "accessToken",
        "--output",
        "tsv",
    ])
}

pub fn get_userid() -> Result<Uuid> {
    let as_str = az_cmd(&[
        "ad",
        "signed-in-user",
        "show",
        "--query",
        "id",
        "--output",
        "tsv",
    ])?;
    Ok(Uuid::parse_str(&as_str)?)
}
