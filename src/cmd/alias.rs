use crate::config::{read_aliases, write_aliases};
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct AliasArgs {
    #[command(subcommand)]
    pub action: AliasAction,
}

#[derive(Subcommand)]
pub enum AliasAction {
    /// Define a bash alias inside the container.
    ///
    /// VALUE can reference container env vars like $MAUDEBOX_INSTANCE — they
    /// expand when the alias is invoked, not when defined. Quote VALUE on the
    /// shell to keep arguments together.
    Add {
        /// Alias name (letters, digits, '_' and '-'; must not start with a digit).
        name: String,
        /// Alias body. Quote this on the shell.
        value: String,
    },
    /// Print all configured aliases.
    List,
    /// Remove a configured alias.
    Rm {
        /// Alias name.
        name: String,
    },
}

pub fn run(action: AliasAction) -> Result<i32> {
    let mut current = read_aliases()?;
    match action {
        AliasAction::List => {
            for (n, v) in &current {
                println!("{n} = {v}");
            }
            Ok(0)
        }
        AliasAction::Add { name, value } => {
            if !is_valid_name(&name) {
                eprintln!(
                    "maudebox alias add: invalid alias name '{name}' (alphanumerics, '_', and '-' only)"
                );
                return Ok(1);
            }
            let existing = current.iter_mut().find(|(n, _)| *n == name);
            let action_word = if let Some((_, v)) = existing {
                if *v == value {
                    println!("Already in the user config: {name} = {value}");
                    return Ok(0);
                }
                *v = value.clone();
                "Updated"
            } else {
                current.push((name.clone(), value.clone()));
                "Added"
            };
            write_aliases(&current)?;
            println!("{action_word}: {name} = {value}");
            Ok(0)
        }
        AliasAction::Rm { name } => {
            let idx = current.iter().position(|(n, _)| *n == name);
            let Some(idx) = idx else {
                eprintln!("Not in the user config: {name}");
                return Ok(1);
            };
            current.remove(idx);
            write_aliases(&current)?;
            println!("Removed: {name}");
            Ok(0)
        }
    }
}

// Same regex shape as bash: ^[a-zA-Z_][a-zA-Z0-9_-]*$
fn is_valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}
