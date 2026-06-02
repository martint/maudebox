// xtask — build orchestration for the maudebox project.
//
// Invoked via the `cargo xtask` alias defined in `.cargo/config.toml`. The
// xtask crate has no dependencies on purpose: it should compile cold in well
// under a second so subcommands feel like cargo built-ins.
//
//   cargo xtask image       # build the docker image
//   cargo xtask all         # build the wrapper binary (release) + the image

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const DEFAULT_MVND_VERSION: &str = "1.0.5";
const DEFAULT_JJ_VERSION: &str = "0.41.0";
const DEFAULT_RUST_VERSION: &str = "1.95.0";
const DEFAULT_BUN_VERSION: &str = "1.3.13";
const DEFAULT_PNPM_VERSION: &str = "11.1.0";
const DEFAULT_CLAUDE_VERSION: &str = "2.1.170";
const DEFAULT_TAG: &str = "maudebox";

const USAGE: &str = "\
Usage: cargo xtask <subcommand> [options]

Subcommands:
  image    Build the maudebox docker image.
  all      Build the wrapper binary (release) and the docker image.
  help     Show this help.

Options for `image` and `all`:
  --mvnd-version VERSION    default: 1.0.5
  --jj-version VERSION      default: 0.41.0
  --rust-version VERSION    default: 1.95.0
  --bun-version VERSION     default: 1.3.13
  --pnpm-version VERSION    default: 11.1.0
  --claude-version VERSION  default: 2.1.170
  --tag TAG                 default: maudebox
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::from(1)
        }
    }
}

fn run(args: &[String]) -> Result<i32, String> {
    let sub = args.first().map(String::as_str).unwrap_or("help");
    let rest = &args.get(1..).unwrap_or(&[][..])[..];
    match sub {
        "image" => cmd_image(rest),
        "all" => cmd_all(rest),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(0)
        }
        other => {
            eprintln!("Unknown subcommand: {other}\n");
            print!("{USAGE}");
            Ok(1)
        }
    }
}

struct ImageOpts {
    mvnd_version: String,
    jj_version: String,
    rust_version: String,
    bun_version: String,
    pnpm_version: String,
    claude_version: String,
    tag: String,
}

fn parse_image_opts(args: &[String]) -> Result<ImageOpts, String> {
    let mut opts = ImageOpts {
        mvnd_version: DEFAULT_MVND_VERSION.into(),
        jj_version: DEFAULT_JJ_VERSION.into(),
        rust_version: DEFAULT_RUST_VERSION.into(),
        bun_version: DEFAULT_BUN_VERSION.into(),
        pnpm_version: DEFAULT_PNPM_VERSION.into(),
        claude_version: DEFAULT_CLAUDE_VERSION.into(),
        tag: DEFAULT_TAG.into(),
    };
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let val = || {
            args.get(i + 1)
                .cloned()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{a}: missing value"))
        };
        match a.as_str() {
            "--mvnd-version" => {
                opts.mvnd_version = val()?;
                i += 2;
            }
            "--jj-version" => {
                opts.jj_version = val()?;
                i += 2;
            }
            "--rust-version" => {
                opts.rust_version = val()?;
                i += 2;
            }
            "--bun-version" => {
                opts.bun_version = val()?;
                i += 2;
            }
            "--pnpm-version" => {
                opts.pnpm_version = val()?;
                i += 2;
            }
            "--claude-version" => {
                opts.claude_version = val()?;
                i += 2;
            }
            "--tag" => {
                opts.tag = val()?;
                i += 2;
            }
            other => return Err(format!("Unknown option: {other}")),
        }
    }
    Ok(opts)
}

fn cmd_image(args: &[String]) -> Result<i32, String> {
    let opts = parse_image_opts(args)?;
    build_image(&opts)
}

fn cmd_all(args: &[String]) -> Result<i32, String> {
    let opts = parse_image_opts(args)?;
    let rc = build_wrapper()?;
    if rc != 0 {
        return Ok(rc);
    }
    build_image(&opts)
}

fn build_wrapper() -> Result<i32, String> {
    println!("==> cargo build --release -p maudebox");
    let status = Command::new(cargo_bin())
        .args(["build", "--release", "-p", "maudebox"])
        .current_dir(workspace_root())
        .status()
        .map_err(|e| format!("spawning cargo: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

fn build_image(opts: &ImageOpts) -> Result<i32, String> {
    let context = workspace_root().join("docker");
    if !context.is_dir() {
        return Err(format!("missing build context: {}", context.display()));
    }
    println!("==> docker build -t {} {}", opts.tag, context.display());
    println!("    mvnd   : {}", opts.mvnd_version);
    println!("    jj     : {}", opts.jj_version);
    println!("    rust   : {}", opts.rust_version);
    println!("    bun    : {}", opts.bun_version);
    println!("    pnpm   : {}", opts.pnpm_version);
    println!("    claude : {}", opts.claude_version);
    let status = Command::new("docker")
        .arg("build")
        .arg("--build-arg")
        .arg(format!("MVND_VERSION={}", opts.mvnd_version))
        .arg("--build-arg")
        .arg(format!("JJ_VERSION={}", opts.jj_version))
        .arg("--build-arg")
        .arg(format!("RUST_VERSION={}", opts.rust_version))
        .arg("--build-arg")
        .arg(format!("BUN_VERSION={}", opts.bun_version))
        .arg("--build-arg")
        .arg(format!("PNPM_VERSION={}", opts.pnpm_version))
        .arg("--build-arg")
        .arg(format!("CLAUDE_VERSION={}", opts.claude_version))
        .arg("-t")
        .arg(&opts.tag)
        .arg(&context)
        .status()
        .map_err(|e| format!("spawning docker: {e}"))?;
    Ok(status.code().unwrap_or(1))
}

// `CARGO` is set by cargo when it invokes us via the alias, so we re-use the
// same toolchain rather than searching $PATH.
fn cargo_bin() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

// xtask's CARGO_MANIFEST_DIR is `<root>/xtask`; the workspace root is its
// parent. Set at compile time, so this is free at runtime.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask must live one level below the workspace root")
        .to_path_buf()
}
