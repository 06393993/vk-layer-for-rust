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

use std::{collections::BTreeSet, path::PathBuf, thread::JoinHandle};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use ignore::{types::TypesBuilder, WalkBuilder};
use log::{error, info, trace};
use xshell::{cmd, Cmd, Shell};

/// Format source files in this repository.
#[derive(Parser)]
struct Cli {
    /// The file types to format. For example markdown. Default to all.
    #[arg(long, value_enum)]
    file_type: Vec<FmtFileType>,

    /// Do not apply changes to files
    #[arg(long)]
    check: bool,
}

impl Cli {
    fn creat_cmd_contexts(&self) -> Result<Vec<Box<dyn CommandContext>>> {
        let file_types = self.file_type.iter().collect::<BTreeSet<_>>();
        let command_context = file_types
            .iter()
            .map(|file_type| -> Result<Vec<Box<dyn CommandContext>>> {
                match file_type {
                    FmtFileType::Markdown => {
                        let md_type = TypesBuilder::new()
                            .add_defaults()
                            .select("md")
                            .build()
                            .context("build the markdown type filter")?;
                        WalkBuilder::new("./")
                            .types(md_type)
                            .build()
                            .filter_map(|res| {
                                let entry = match res {
                                    Err(e) => {
                                        error!("{}", e);
                                        return None;
                                    }
                                    Ok(entry) => entry,
                                };
                                let is_file = entry
                                    .file_type()
                                    .map(|file_type| file_type.is_file())
                                    .unwrap_or(false);
                                if !is_file {
                                    trace!("Skip directory {}", entry.path().display());
                                    return None;
                                }
                                Some(entry.path().to_owned())
                            })
                            .map(|file| -> Result<Box<dyn CommandContext>> {
                                let sh = Shell::new()
                                    .context("create shell for format command contextx")?;
                                #[cfg(windows)]
                                let end_of_line = "crlf";
                                #[cfg(unix)]
                                let end_of_line = "lf";
                                Ok(Box::new(MarkdownFormatCommandContext {
                                    file,
                                    end_of_line: end_of_line.to_owned(),
                                    wrap: 100,
                                    sh,
                                    check: self.check,
                                }))
                            })
                            .collect()
                    }
                }
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(command_context.into_iter().flatten().collect())
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
enum FmtFileType {
    /// Format markdown files.
    Markdown,
}

impl FmtFileType {
    fn all() -> Vec<Self> {
        use FmtFileType::*;
        vec![Markdown]
    }
}

trait CommandContext: Send {
    fn build_commands(&self) -> Vec<Cmd<'_>>;
}

struct MarkdownFormatCommandContext {
    file: PathBuf,
    end_of_line: String,
    wrap: u32,
    sh: Shell,
    check: bool,
}

impl CommandContext for MarkdownFormatCommandContext {
    fn build_commands(&self) -> Vec<Cmd<'_>> {
        let file = &self.file;
        let wrap = self.wrap.to_string();
        let eol = &self.end_of_line;
        let check_arg = if self.check { "--check" } else { "" };
        vec![cmd!(
            self.sh,
            "pipenv run mdformat --wrap {wrap} --end-of-line {eol} {file} {check_arg}"
        )]
    }
}

fn main() -> Result<()> {
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .filter_module("bindgen", log::LevelFilter::Error)
        .init();

    let cli = {
        let mut cli = Cli::parse();
        if cli.file_type.is_empty() {
            cli.file_type = FmtFileType::all();
        }
        cli
    };

    info!("Start building the format command contexts.");
    let fmt_cmd_contexts = cli
        .creat_cmd_contexts()
        .context("create format command contexts")?;
    info!("Building the format command contexts completes.");

    let join_handles: Vec<JoinHandle<Result<()>>> = fmt_cmd_contexts
        .into_iter()
        .map(|fmt_cmd_context| {
            std::thread::spawn(move || -> Result<()> {
                let cmds = fmt_cmd_context.build_commands();
                for cmd in cmds.into_iter() {
                    let cmd_display = format!("{}", cmd);
                    info!("Running: `{}'", cmd_display);
                    cmd.quiet().run()?;
                    info!("Completed: `{}'", cmd_display);
                }
                Ok(())
            })
        })
        .collect::<Vec<_>>();
    join_handles
        .into_iter()
        .map(|join_handle| -> Result<()> {
            match join_handle.join() {
                Ok(res) => res,
                Err(e) => std::panic::resume_unwind(e),
            }
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(())
}
