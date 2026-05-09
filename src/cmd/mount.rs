use crate::config::{read_mounts, write_mounts};
use crate::mount::{mount_spec_key, parse_mount_spec};
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct MountArgs {
    #[command(subcommand)]
    pub action: MountAction,
}

#[derive(Subcommand)]
pub enum MountAction {
    /// Append a mount spec (HOST:CONTAINER[:ro|rw|overlay]) to the user config.
    Add {
        /// HOST:CONTAINER[:ro|rw|overlay]
        spec: String,
    },
    /// Print all configured mount specs.
    List,
    /// Remove a configured mount spec.
    Rm {
        /// HOST:CONTAINER[:ro|rw|overlay]
        spec: String,
    },
}

pub fn run(action: MountAction) -> Result<i32> {
    let current = read_mounts()?;
    match action {
        MountAction::List => {
            for s in &current {
                println!("{s}");
            }
            Ok(0)
        }
        MountAction::Add { spec } => {
            if let Err(e) = parse_mount_spec(&spec) {
                eprintln!("{e}");
                return Ok(1);
            }
            let new_key = mount_spec_key(&spec);
            if let Some(dup) = current.iter().find(|s| mount_spec_key(s) == new_key) {
                println!("Already in the user config: {dup}");
                return Ok(0);
            }
            let mut next = current;
            next.push(spec.clone());
            write_mounts(&next)?;
            println!("Added: {spec}");
            Ok(0)
        }
        MountAction::Rm { spec } => {
            let target_key = mount_spec_key(&spec);
            let idx = current.iter().position(|s| mount_spec_key(s) == target_key);
            let Some(idx) = idx else {
                eprintln!("Not in the user config: {spec}");
                return Ok(1);
            };
            let mut next = current;
            next.remove(idx);
            write_mounts(&next)?;
            println!("Removed: {spec}");
            Ok(0)
        }
    }
}
