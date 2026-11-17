//! `kustomize-rs build <dir>`: a thin CLI over the library, for trying the
//! engine against real repositories. Not a replacement for the kustomize CLI.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use kustomize::fs::DiskFs;
use kustomize::{BuildOptions, LoadRestrictions, Reorder, build};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the resources a kustomization directory produces.
    Build {
        /// Directory containing a kustomization file.
        #[arg(default_value = ".")]
        dir: PathBuf,
        /// Where files may be loaded from (same values as kustomize).
        #[arg(long, value_enum, default_value_t = Restrictor::LoadRestrictionsRootOnly)]
        load_restrictor: Restrictor,
        /// Use the settings Flux's kustomize-controller builds with.
        #[arg(long)]
        flux: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
#[value(rename_all = "verbatim")]
enum Restrictor {
    LoadRestrictionsRootOnly,
    LoadRestrictionsNone,
}

fn main() -> ExitCode {
    let Command::Build {
        dir,
        load_restrictor,
        flux,
    } = Cli::parse().command;
    let options = if flux {
        BuildOptions::flux()
    } else {
        BuildOptions {
            load_restrictions: match load_restrictor {
                Restrictor::LoadRestrictionsRootOnly => LoadRestrictions::RootOnly,
                Restrictor::LoadRestrictionsNone => LoadRestrictions::None,
            },
            reorder: Reorder::None,
            add_managed_by_label: false,
        }
    };
    match build(&DiskFs::unrestricted(), &dir, &options).and_then(|m| m.to_yaml()) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Error: {e}");
            ExitCode::FAILURE
        }
    }
}
