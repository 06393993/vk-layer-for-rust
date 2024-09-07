// Copyright 2024 Google LLC
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

use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context};
use clap::{Error as ClapError, Parser};
use log::{debug, error};
use std::borrow::Borrow;
use std::io;

const ABOUT: &str = "List files and forward the canonical path as the arguments. If the command \
                     failed, the exit code will be non-zero. If a file can't be accessed, the \
                     file will be skipped instead of triggering an error.";

#[derive(Parser, Debug)]
#[command(about = ABOUT)]
pub struct Cli {
    /// Globs to include in search. Paths that match any of include patterns will be included. If
    /// no include pattern is provided, all files will be included.
    #[arg(long)]
    pub include: Vec<String>,

    /// Matching is done relative to this directory path provided. .gitignore is also found from
    /// this directory.
    #[arg(long, default_value_os_t = Cli::default_match_base())]
    pub match_base: OsString,

    /// Search root.
    pub root: OsString,

    /// Command to run on the matching files.
    #[arg(last = true)]
    pub command: Vec<String>,
}

impl Cli {
    fn default_match_base() -> OsString {
        let mut path = PathBuf::from(
            env::var("CARGO_MANIFEST_DIR")
                .expect("CARGO_MANIFEST_DIR should always be set if run with cargo"),
        );
        path.pop();
        path.into()
    }

    pub fn get_paths(&self) -> anyhow::Result<impl IntoIterator<Item = PathBuf>> {
        let mut overrides = ignore::overrides::OverrideBuilder::new(&self.match_base);
        for include in &self.include {
            overrides
                .add(include)
                .context("add include to ignore overrides")?;
        }
        let overrides = overrides.build().context("build the ignore overrides")?;
        let mut builder = ignore::WalkBuilder::new(&self.root);
        builder.overrides(overrides);
        let prettier_ignore = PathBuf::from(&self.match_base).join(".prettierignore");
        builder.add_custom_ignore_filename(prettier_ignore);
        let gitignore = PathBuf::from(&self.match_base).join(".gitignore");
        if let Some(e) = builder.add_ignore(gitignore) {
            return Err(e).context("adding .gitignore to ignore::WalkBuilder");
        }
        Ok(builder.build().filter_map({
            let match_base = self.match_base.clone();
            move |res| {
                let entry = match res {
                    Ok(entry) => entry,
                    Err(e) => {
                        error!("Failed to handle a file entry: {:?}", e);
                        return None;
                    }
                };
                if entry.file_type().map(|file_type| file_type.is_file()) != Some(true) {
                    return None;
                }
                let mut path = entry.into_path();
                if path.is_relative() {
                    path = PathBuf::from(&match_base).join(path);
                }
                path = path.canonicalize().expect("the path should be valid");
                Some(path)
            }
        }))
    }
}

pub fn parse_args<T>(args: impl IntoIterator<Item = T>) -> Result<Cli, ClapError>
where
    T: Into<OsString> + Clone,
{
    Cli::try_parse_from(args)
}

pub struct TaskBuilder {
    command: Vec<String>,
}

impl TaskBuilder {
    pub fn new(command: Vec<String>) -> Self {
        Self { command }
    }

    pub fn build_task(&self, paths: impl IntoIterator<Item = impl AsRef<Path>>) -> Task {
        if self.command.is_empty() {
            let mut to_print = OsString::new();
            let mut first = true;
            for path in paths {
                if first {
                    first = false;
                } else {
                    to_print.push(" ");
                }
                to_print.push(path.as_ref());
            }
            Task::Print(to_print)
        } else {
            let mut command = std::process::Command::new(&self.command[0]);
            for arg in self.command.iter().skip(1) {
                command.arg(arg);
            }
            for path in paths {
                command.arg(path.as_ref());
            }
            Task::SpawnProcess(command)
        }
    }
}

pub enum Task {
    Print(OsString),
    SpawnProcess(std::process::Command),
}

impl Task {
    pub fn run(
        self,
        stdout_buffer: &mut impl io::Write,
        stderr_buffer: &mut impl io::Write,
    ) -> anyhow::Result<()> {
        match self {
            Self::Print(to_print) => {
                let to_print = to_print.to_string_lossy();
                let to_print: &str = to_print.borrow();
                writeln!(stdout_buffer, "{}", to_print).context("print matched paths")
            }
            Self::SpawnProcess(mut command) => {
                let command_str = {
                    let mut command_strs =
                        vec![command.get_program().to_string_lossy().to_string()];
                    for arg in command.get_args() {
                        command_strs.push(arg.to_string_lossy().to_string());
                    }
                    let command = shlex::try_join(command_strs.iter().map(String::as_ref))
                        .expect("should not fail to join");
                    command
                };
                debug!("start command: `{}'", command_str);
                let output = command
                    .output()
                    .with_context(|| format!("spawn and wait for the command {}", &command_str))?;
                ensure!(output.status.success(), {
                    match &output.status.code() {
                        Some(exit_code) => format!(
                            "The process {} failed with exit code {}.",
                            command_str, exit_code
                        ),
                        None => format!("The process {} failed without an exit code.", command_str),
                    }
                });
                if let Err(e) = stdout_buffer.write_all(&output.stdout) {
                    error!(
                        "Failed to write the stdout for process {}: {:?}",
                        command_str, e
                    );
                }
                if let Err(e) = stderr_buffer.write_all(&output.stderr) {
                    error!(
                        "Failed to write the stderr for process {}: {:?}",
                        command_str, e
                    );
                }
                debug!("command ends successfully: `{}'", command_str);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_args() {
        let args = [
            "progname",
            "--include=*.yaml",
            "--include",
            "*.yml",
            "tmp/subdir",
            "--",
            "ls",
            "-lah",
        ];
        let cli = parse_args(args).expect("should parse successfully");
        assert_eq!(cli.include, vec!["*.yaml", "*.yml"]);
        assert_eq!(cli.command, vec!["ls", "-lah"]);
        assert_eq!(cli.root, "tmp/subdir");
    }

    #[test]
    fn test_parse_args_missing_root() {
        let res = parse_args(["progname"]);
        assert!(res.is_err());
    }
}
