// Copyright 2023 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};

mod ci;
mod codegen;
mod fmt;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// CI related commands.
    Ci(CiCli),

    /// Regenerate source.
    Codegen,

    /// Format source code.
    Fmt(FmtCli),
}

#[derive(Args)]
struct CiCli {
    /// The path to the output json file.
    #[arg(long)]
    output_path: PathBuf,

    /// The left text of the coverage badge.
    #[arg(long)]
    label: String,

    /// The json field name and the path to the json coverage report separated by colon.
    coverage_report: PathBuf,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum FmtFileType {
    /// Format markdown files.
    Markdown,
}

#[derive(Args)]
struct FmtCli {
    /// The file types to format. For example markdown. Default to all.
    #[arg(long, value_enum)]
    file_type: Vec<FmtFileType>,

    /// Do not apply changes to files
    #[arg(long)]
    check: bool,
}

fn main() -> Result<()> {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .filter_module("bindgen", log::LevelFilter::Error)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Ci(ci_cli) => ci::main(ci_cli),
        Commands::Codegen => codegen::main(),
        Commands::Fmt(fmt_cli) => fmt::main(fmt_cli),
    }
}
